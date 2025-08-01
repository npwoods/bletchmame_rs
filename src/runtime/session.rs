use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::Error;
use anyhow::Result;
use itertools::Itertools;
use slint::invoke_from_event_loop;
use tracing::Level;
use tracing::info;
use tracing::span;

use crate::appcommand::AppCommand;
use crate::console::Console;
use crate::console::EmitType;
use crate::job::Job;
use crate::platform::CommandExt;
use crate::prefs::PrefsPaths;
use crate::runtime::MameStderr;
use crate::runtime::MameWindowing;
use crate::runtime::args::MameArguments;
use crate::runtime::command::MameCommand;
use crate::status::Update;
use crate::threadlocalbubble::ThreadLocalBubble;

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("MAME Error Response: {0:?}")]
	MameErrorResponse(String),
	#[error("Unexpected Response from MAME: {0:?}")]
	MameResponseNotUnderstood(String),
	#[error("Unexpected EOF from MAME: {0}")]
	EofFromMame(String),
}

#[derive(Debug)]
enum MameEvent {
	SessionEnded,
	StatusUpdate(Update),
}

pub fn spawn_mame_session_thread(
	prefs_paths: &PrefsPaths,
	mame_windowing: &MameWindowing,
	mame_stderr: MameStderr,
	console: Arc<Mutex<Option<Console>>>,
	callback: Rc<dyn Fn(AppCommand) + 'static>,
) -> (Job<Result<()>>, Sender<MameCommand>) {
	let callback_bubble = ThreadLocalBubble::new(callback);
	let event_callback = move |event| {
		let callback_bubble = callback_bubble.clone();
		invoke_from_event_loop(move || {
			let command = match event {
				MameEvent::SessionEnded => AppCommand::MameSessionEnded,
				MameEvent::StatusUpdate(update) => AppCommand::MameStatusUpdate(update),
			};
			(callback_bubble.unwrap())(command)
		})
		.unwrap();
	};
	let mame_args = MameArguments::new(prefs_paths, mame_windowing);
	let (sender, receiver) = channel();

	let job = Job::new(move || execute_mame(&mame_args, &receiver, &event_callback, mame_stderr, console.as_ref()));
	(job, sender)
}

fn execute_mame(
	mame_args: &MameArguments,
	receiver: &Receiver<MameCommand>,
	event_callback: &impl Fn(MameEvent),
	mame_stderr: MameStderr,
	console: &Mutex<Option<Console>>,
) -> Result<()> {
	let span = span!(Level::INFO, "execute_mame");
	let _guard = span.enter();

	// launch MAME, launch!
	info!(?mame_args, "Launching MAME");
	let args = mame_args.args.iter().map(OsStr::new);
	let (mame_stderr, create_no_window_flag) = match mame_stderr {
		MameStderr::Capture => (Stdio::piped(), true),
		MameStderr::Inherit => (Stdio::inherit(), false),
	};
	emit_console_command_line(console, mame_args);
	let mut child = Command::new(&mame_args.program)
		.args(args.clone())
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(mame_stderr)
		.create_no_window(create_no_window_flag)
		.spawn()
		.map_err(|error| Error::new(error).context("Error launching MAME"))?;

	// interact with MAME, do our thing
	let mame_result = interact_with_mame(&mut child, receiver, console, &event_callback);

	// if we either errored, try to kill the process
	if mame_result.is_err() {
		let _ = child.kill();
	}

	// await the exit status
	let exit_status = child.wait();
	info!(?exit_status, "MAME exited");

	// notify the host that the session has ended
	event_callback(MameEvent::SessionEnded);

	// and we're done
	mame_result
}

fn interact_with_mame(
	child: &mut Child,
	receiver: &Receiver<MameCommand>,
	console: &Mutex<Option<Console>>,
	event_callback: &impl Fn(MameEvent),
) -> Result<()> {
	// set up what we need to interact with MAME as a child process
	let mut mame_stdin = BufWriter::new(child.stdin.take().unwrap());
	let mut mame_stderr = child.stderr.take().map(BufReader::new);
	let mut mame_stdout = BufReader::new(child.stdout.take().unwrap());
	let mut line = String::new();
	let mut is_exiting = false;
	let mut is_running = false;

	loop {
		info!("Calling read_response_from_mame()");
		let (update, is_signal) = read_response_from_mame(&mut mame_stdout, &mut mame_stderr, console, &mut line)?;

		if let Some(update) = update {
			is_running = update.is_running();
			event_callback(MameEvent::StatusUpdate(update))
		}

		if is_signal {
			if is_exiting {
				break Ok(());
			}
			is_exiting = match process_event_from_front_end(receiver, &mut mame_stdin, is_running, console) {
				Ok(x) => x,
				Err(e) => break Err(e),
			};
		}
	}
}

fn read_response_from_mame(
	mame_stdout: &mut impl BufRead,
	mame_stderr: &mut Option<impl BufRead>,
	console: &Mutex<Option<Console>>,
	line: &mut String,
) -> Result<(Option<Update>, bool)> {
	#[derive(Debug, Clone, Copy, PartialEq)]
	enum ResponseLine {
		Ok,
		OkStatus,
		Info,
		Cruft,
	}

	let (resp, comment) = match read_line_from_mame(mame_stdout, mame_stderr, line) {
		Ok(()) => {
			let line_without_eolns = line.trim_end_matches(&['\r', '\n'][..]);
			if let Some(status_line) = line.strip_prefix("@") {
				emit_console(console, EmitType::Response, line_without_eolns);
				let (msg, comment) = if let Some((msg, comment)) = status_line.split_once("###") {
					(msg.trim_end(), Some(comment.trim()))
				} else {
					(status_line.trim_end(), None)
				};

				let result = match msg {
					"OK" => Ok(ResponseLine::Ok),
					"OK STATUS" => Ok(ResponseLine::OkStatus),
					"INFO" => Ok(ResponseLine::Info),
					"ERROR" => Err(ThisError::MameErrorResponse(comment.unwrap_or_default().to_string()).into()),
					_ => Err(ThisError::MameResponseNotUnderstood(line.to_string()).into()),
				};

				(result, comment)
			} else {
				emit_console(console, EmitType::Cruft, line_without_eolns);
				(Ok(ResponseLine::Cruft), Some(line.as_str()))
			}
		}
		Err(e) => (Err(e), None),
	};

	info!(resp=?resp, comment=?comment);
	let resp = resp?;

	let update = if resp == ResponseLine::OkStatus {
		// read the status XML from MAME
		info!("Starting to parse update");
		let update = Update::parse(&mut *mame_stdout);
		info!("update" = ?update.as_ref().map(|_| ()), "Parsed update");

		// read until end of line
		let result = read_line_from_mame(mame_stdout, mame_stderr, line);
		info!(?line, ?result, "Poststatus eoln");
		result?;
		if !line.trim().is_empty() {
			return Err(ThisError::MameResponseNotUnderstood(line.to_string()).into());
		}

		// bail if we errored
		Some(update?)
	} else {
		None
	};

	// is the response a "signal", indicating that it is our turn to issue a command?
	let is_signal = match resp {
		ResponseLine::Ok | ResponseLine::OkStatus => true,
		ResponseLine::Info | ResponseLine::Cruft => false,
	};

	Ok((update, is_signal))
}

fn read_line_from_mame(
	mame_stdout: &mut impl BufRead,
	mame_stderr: &mut Option<impl BufRead>,
	line: &mut String,
) -> Result<()> {
	line.clear();
	match mame_stdout.read_line(line) {
		Ok(0) => {
			let mame_stderr_text = mame_stderr.as_mut().map(read_text_from_reader).unwrap_or_default();
			Err(ThisError::EofFromMame(mame_stderr_text).into())
		}
		Ok(_) => Ok(()),
		Err(error) => Err(Error::new(error).context("Error reading from MAME")),
	}
}

fn read_text_from_reader(read: &mut impl Read) -> String {
	let mut buf = Vec::new();
	if read.read_to_end(&mut buf).is_err() {
		buf.clear();
	}
	String::from_utf8(buf).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).to_string())
}

fn process_event_from_front_end(
	receiver: &Receiver<MameCommand>,
	mame_stdin: &mut BufWriter<impl Write>,
	is_running: bool,
	console: &Mutex<Option<Console>>,
) -> Result<bool> {
	let timeout = if is_running {
		Duration::from_secs(1)
	} else {
		Duration::from_secs(10)
	};
	let (command, is_exit) = match receiver.recv_timeout(timeout) {
		Ok(command) => (command, false),
		Err(RecvTimeoutError::Timeout) => (MameCommand::ping(), false),
		Err(RecvTimeoutError::Disconnected) => (MameCommand::exit(), true),
	};

	info!(?command);

	fn mame_write_err(e: impl Into<Error>) -> Error {
		e.into().context("Error writing to MAME")
	}

	emit_console(console, EmitType::Command, command.text());
	writeln!(mame_stdin, "{}", command.text()).map_err(mame_write_err)?;
	mame_stdin.flush().map_err(mame_write_err)?;

	Ok(is_exit)
}

// emits a line to an active console, if present
fn emit_console(console: &Mutex<Option<Console>>, emit_type: EmitType, s: &str) {
	with_active_console(console, |console| console.emit(emit_type, s));
}

fn emit_console_command_line(console: &Mutex<Option<Console>>, mame_args: &MameArguments) {
	with_active_console(console, |console| {
		let args = mame_args.args.iter().map(|x| x.to_string_lossy());
		let text = std::iter::once(Cow::Borrowed(mame_args.program.as_str()))
			.chain(args)
			.map(|s| {
				if s.is_empty() || s.contains(' ') {
					Cow::Owned(format!("\"{s}\""))
				} else {
					s
				}
			})
			.join(" ");
		console.emit(EmitType::CommandLine, &text)
	});
}

fn with_active_console(console: &Mutex<Option<Console>>, f: impl FnOnce(&mut Console) -> Result<()>) {
	let mut console = console.lock().unwrap();
	if console.as_mut().is_none_or(|console| f(console).is_err()) {
		*console = None;
	}
}
