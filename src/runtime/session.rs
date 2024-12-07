use std::borrow::Cow;
use std::cell::Cell;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::spawn;
use std::thread::JoinHandle;

use anyhow::Error;
use anyhow::Result;
use blockingqueue::BlockingQueue;
use itertools::Itertools;
use tracing::event;
use tracing::Level;

use crate::platform::CommandExt;
use crate::runtime::args::MameArguments;
use crate::runtime::MameCommand;
use crate::runtime::MameEvent;
use crate::runtime::MameStderr;
use crate::status::Update;

const LOG: Level = Level::DEBUG;

pub struct MameSession {
	handle: JoinHandle<()>,
	comm: Arc<SessionCommunication>,
	exit_issued: Cell<bool>,
}

struct SessionCommunication {
	message_queue: BlockingQueue<ProcessedCommand>,
	message_queue_len: AtomicU64,
	mame_pid: AtomicU64,
}

#[derive(Debug)]
struct ProcessedCommand {
	pub text: Cow<'static, str>,
	pub is_exit: bool,
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("MAME Error Response: {0:?}")]
	MameErrorResponse(String),
	#[error("Problems found during MAME preflight: {0:?}")]
	MameResponseNotUnderstood(String),
	#[error("Unexpected EOF from MAME: {0}")]
	EofFromMame(String),
}

impl MameSession {
	pub fn new(
		mame_args: MameArguments,
		event_callback: impl Fn(MameEvent) + Send + 'static,
		mame_stderr: MameStderr,
	) -> Self {
		// prepare communication with the child
		let comm = SessionCommunication {
			message_queue: BlockingQueue::new(),
			mame_pid: (!0).into(),
			message_queue_len: 0.into(),
		};
		let comm = Arc::new(comm);

		// and start the thread
		let comm_clone = comm.clone();
		let handle = spawn(move || thread_proc(&mame_args, &comm_clone, event_callback, mame_stderr));

		// and set ourselves up
		Self {
			handle,
			comm,
			exit_issued: Cell::new(false),
		}
	}

	pub fn has_pending_commands(&self) -> bool {
		self.comm.message_queue_len.load(Ordering::Relaxed) > 0
	}

	pub fn issue_command(&self, command: MameCommand) {
		if command == MameCommand::Exit {
			self.exit_issued.set(true);
		}
		self.comm.message_queue.push(command.into());
		self.comm.message_queue_len.fetch_add(1, Ordering::Relaxed);
	}

	pub fn shutdown(self) {
		if !self.exit_issued.get() {
			self.issue_command(MameCommand::Exit);
		}
		self.handle.join().unwrap()
	}
}
impl From<MameCommand<'_>> for ProcessedCommand {
	fn from(value: MameCommand<'_>) -> Self {
		let text = command_text(&value);
		let is_exit = value == MameCommand::Exit;
		ProcessedCommand { text, is_exit }
	}
}

fn thread_proc(
	mame_args: &MameArguments,
	comm: &SessionCommunication,
	event_callback: impl Fn(MameEvent),
	mame_stderr: MameStderr,
) {
	event_callback(MameEvent::SessionStarted);
	if let Err(e) = execute_mame(mame_args, comm, &event_callback, mame_stderr) {
		event_callback(MameEvent::Error(e));
	}
	event_callback(MameEvent::SessionEnded);
}

fn execute_mame(
	mame_args: &MameArguments,
	comm: &SessionCommunication,
	event_callback: &impl Fn(MameEvent),
	mame_stderr: MameStderr,
) -> Result<()> {
	// launch MAME, launch!
	event!(LOG, "execute_mame(): Launching MAME: mame_args={mame_args:?}");
	let args = mame_args.args.iter().map(|x| x.as_ref());
	let (mame_stderr, create_no_window_flag) = match mame_stderr {
		MameStderr::Capture => (Stdio::piped(), true),
		MameStderr::Inherit => (Stdio::inherit(), false),
	};
	let mut child = Command::new(&mame_args.program)
		.args(args)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(mame_stderr)
		.create_no_window(create_no_window_flag)
		.spawn()
		.map_err(|error| Error::new(error).context("Error launching MAME"))?;

	// MAME launched!  we now have a pid
	comm.mame_pid.store(child.id().into(), Ordering::Relaxed);

	// interact with MAME, do our thing
	let mame_result = interact_with_mame(&mut child, comm, &event_callback);

	// await the exit status
	let exit_status = child.wait();
	event!(LOG, "execute_mame(): MAME exited exit_status={:?}", exit_status);

	// and we're done
	comm.mame_pid.store(!0, Ordering::Relaxed);
	mame_result
}

fn interact_with_mame(
	child: &mut Child,
	comm: &SessionCommunication,
	event_callback: &impl Fn(MameEvent),
) -> Result<()> {
	// set up what we need to interact with MAME as a child process
	let mut mame_stdin = BufWriter::new(child.stdin.take().unwrap());
	let mut mame_stderr = child.stderr.take().map(BufReader::new);
	let mut mame_stdout = BufReader::new(child.stdout.take().unwrap());
	let mut line = String::new();
	let mut is_exiting = false;

	loop {
		event!(LOG, "interact_with_mame(): calling read_line_from_mame()");
		let (update, is_signal) = read_response_from_mame(&mut mame_stdout, &mut mame_stderr, &mut line)?;

		if let Some(update) = update {
			event_callback(MameEvent::StatusUpdate(update))
		}

		if is_signal {
			if is_exiting {
				break Ok(());
			}
			is_exiting = match process_event_from_front_end(comm, &mut mame_stdin) {
				Ok(x) => x,
				Err(e) => break Err(e),
			};
		}
	}
}

fn read_response_from_mame(
	mame_stdout: &mut impl BufRead,
	mame_stderr: &mut Option<impl BufRead>,
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
			if let Some(status_line) = line.strip_prefix("@") {
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
				(Ok(ResponseLine::Cruft), Some(line.as_str()))
			}
		}
		Err(e) => (Err(e), None),
	};

	event!(LOG, "read_response_from_mame(): resp={:?} comment={:?}", resp, comment);
	let resp = resp?;

	let update = if resp == ResponseLine::OkStatus {
		// read the status XML from MAME
		event!(LOG, "thread_proc(): starting to parse update");
		let update = Update::parse(&mut *mame_stdout);
		event!(LOG, "thread_proc(): parsed update: {:?}", update.as_ref().map(|_| ()));

		// read until end of line
		let result = read_line_from_mame(mame_stdout, mame_stderr, line);
		event!(
			LOG,
			"thread_proc(): poststatus eoln: line={:?} result={:?}",
			line,
			result
		);
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

fn process_event_from_front_end(comm: &SessionCommunication, mame_stdin: &mut BufWriter<impl Write>) -> Result<bool> {
	let command = comm.message_queue.pop();
	comm.message_queue_len.fetch_sub(1, Ordering::Relaxed);
	event!(LOG, "process_event_from_front_end(): command=\"{:?}\"", command);

	fn mame_write_err(e: impl Into<Error>) -> Error {
		e.into().context("Error writing to MAME")
	}

	writeln!(mame_stdin, "{}", command.text).map_err(mame_write_err)?;
	mame_stdin.flush().map_err(mame_write_err)?;

	Ok(command.is_exit)
}

fn command_text(command: &MameCommand<'_>) -> Cow<'static, str> {
	match command {
		MameCommand::Exit => "EXIT".into(),
		MameCommand::Start {
			machine_name,
			initial_loads,
		} => ["START", machine_name]
			.into_iter()
			.chain(initial_loads.iter().flat_map(|(dev, target)| [*dev, *target]))
			.join(" ")
			.into(),
		MameCommand::Stop => "STOP".into(),
		MameCommand::SoftReset => "SOFT_RESET".into(),
		MameCommand::HardReset => "HARD_RESET".into(),
		MameCommand::Pause => "PAUSE".into(),
		MameCommand::Resume => "RESUME".into(),
		MameCommand::Ping => "PING".into(),
		MameCommand::Throttled(throttled) => format!("THROTTLED {}", bool_str(*throttled)).into(),
		MameCommand::ThrottleRate(throttle) => format!("THROTTLE_RATE {}", throttle).into(),
		MameCommand::SetAttenuation(attenuation) => format!("SET_ATTENUATION {}", attenuation).into(),
	}
}

fn bool_str(b: bool) -> &'static str {
	if b {
		"true"
	} else {
		"false"
	}
}
