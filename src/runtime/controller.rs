use std::borrow::Cow;
use std::cell::RefCell;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::spawn;
use std::thread::JoinHandle;

use blockingqueue::BlockingQueue;
use tracing::event;
use tracing::Level;

use crate::prefs::PrefsPaths;
use crate::runtime::args::MameArguments;
use crate::runtime::args::MameArgumentsSource;
use crate::runtime::MameWindowing;
use crate::status::Update;
use crate::Error;
use crate::Result;

const LOG: Level = Level::DEBUG;

pub struct MameController {
	session: RefCell<Option<Session>>,
	event_callback: RefCell<Arc<dyn Fn(MameEvent) + Send + Sync + 'static>>,
}

struct Session {
	handle: JoinHandle<()>,
	comm: Arc<SessionCommunication>,
}

struct SessionCommunication {
	message_queue: BlockingQueue<ProcessedCommand>,
	message_queue_len: AtomicU64,
	mame_pid: AtomicU64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MameCommand<'a> {
	Exit,
	Start {
		machine_name: &'a str,
		software_name: Option<&'a str>,
	},
	Stop,
	Pause,
	Resume,
	Ping,
}

#[derive(Debug)]
pub enum MameEvent {
	SessionStarted,
	SessionEnded,
	Error(Error),
	StatusUpdate(Update),
}

#[derive(Debug)]
struct ProcessedCommand {
	pub text: Cow<'static, str>,
	pub is_exit: bool,
}

impl MameController {
	pub fn new() -> Self {
		Self {
			session: RefCell::new(None),
			event_callback: RefCell::new(Arc::new(|_| {})),
		}
	}

	pub fn set_event_callback(&self, event_callback: impl Fn(MameEvent) + Send + Sync + 'static) {
		self.event_callback.replace(Arc::new(event_callback));
	}

	pub fn has_session(&self) -> bool {
		self.session.borrow().is_some()
	}

	pub fn is_queue_empty(&self) -> bool {
		self.session
			.borrow()
			.as_ref()
			.is_some_and(|session| session.comm.message_queue_len.load(Ordering::Relaxed) == 0)
	}

	pub fn reset(&self, prefs_paths: Option<&PrefsPaths>, mame_windowing: &MameWindowing) {
		// first and foremost, determine if we actually have enough set up to invoke MAME
		let mame_args = prefs_paths.and_then(|prefs_paths| {
			MameArgumentsSource::from_prefs(prefs_paths, mame_windowing)
				.ok()
				.and_then(|x| x.preflight().is_ok().then_some(x))
		});

		// logging
		let prefs_paths_str = if prefs_paths.is_some() { "Some(...)" } else { "None" };
		event!(LOG, "MameController::reset(): prefs_paths={}", prefs_paths_str);

		// is there an active session? if so, join it
		if let Some(session) = self.session.take() {
			session.handle.join().unwrap();
		}

		// are we starting up a new session?
		if let Some(mame_args) = mame_args {
			// we are - we need to communicate with the child
			let comm = SessionCommunication {
				message_queue: BlockingQueue::new(),
				mame_pid: (!0).into(),
				message_queue_len: 0.into(),
			};
			let comm = Arc::new(comm);

			// we also need to prepare the actual command line arguments for MAME
			let mame_args = mame_args.into();

			// and start the thread
			let comm_clone = comm.clone();
			let event_callback = self.event_callback.borrow().clone();
			let handle = spawn(move || thread_proc(&mame_args, &comm_clone, event_callback.as_ref()));

			// and set up our session info
			let session = Session { handle, comm };
			self.session.replace(Some(session));
		}
	}

	pub fn issue_command(&self, command: MameCommand) {
		let session = self.session.borrow();
		let Some(session) = session.as_ref() else {
			event!(LOG, "MameController::issue_command():  No session: {:?}", command);
			return;
		};
		session.comm.message_queue.push(command.into());
		session.comm.message_queue_len.fetch_add(1, Ordering::Relaxed);
	}
}

impl From<MameCommand<'_>> for ProcessedCommand {
	fn from(value: MameCommand<'_>) -> Self {
		let text = command_text(&value);
		let is_exit = value == MameCommand::Exit;
		ProcessedCommand { text, is_exit }
	}
}

fn thread_proc(mame_args: &MameArguments, comm: &SessionCommunication, event_callback: &dyn Fn(MameEvent)) {
	event_callback(MameEvent::SessionStarted);

	if let Err(e) = internal_thread_proc(mame_args, comm, event_callback) {
		event_callback(MameEvent::Error(*e))
	}

	comm.mame_pid.store(!0, Ordering::Relaxed);
	event_callback(MameEvent::SessionEnded);
}

fn internal_thread_proc(
	mame_args: &MameArguments,
	comm: &SessionCommunication,
	event_callback: &dyn Fn(MameEvent),
) -> Result<()> {
	event!(LOG, "thread_proc(): Launching MAME: mame_args={mame_args:?}");

	// launch MAME, launch!
	let args = mame_args.args.iter().map(|x| x.as_ref());
	let mut child = Command::new(&mame_args.program)
		.args(args)
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.map_err(|e| Error::MameLaunch(Box::new(e).into()))?;

	// MAME launched!  we now have a pid
	comm.mame_pid.store(child.id().into(), Ordering::Relaxed);

	// set up what we need to interact with MAME as a child process
	let mut mame_stdin = BufWriter::new(child.stdin.take().unwrap());
	let mut mame_stderr = BufReader::new(child.stderr.take().unwrap());
	let mut mame_stdout = BufReader::new(child.stdout.take().unwrap());
	let mut line = String::new();
	let mut is_exiting = false;

	loop {
		event!(LOG, "thread_proc(): calling read_line_from_mame()");
		let (update, is_cruft) = match read_response_from_mame(&mut mame_stdout, &mut mame_stderr, &mut line) {
			Ok((update, is_cruft)) => (update, is_cruft),
			Err(e) => break Err(e),
		};

		if let Some(update) = update {
			event_callback(MameEvent::StatusUpdate(update))
		}

		if !is_cruft {
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
	mame_stderr: &mut impl BufRead,
	line: &mut String,
) -> Result<(Option<Update>, bool)> {
	#[derive(Debug, Clone, Copy, PartialEq)]
	enum ResponseLine {
		Ok,
		OkStatus,
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
					"ERROR" => Err(Error::MameErrorResponse(comment.unwrap_or_default().to_string()).into()),
					_ => Err(Error::MameResponseNotUnderstood(line.to_string()).into()),
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
			return Err(Error::MameResponseNotUnderstood(line.to_string()).into());
		}

		// bail if we errored
		Some(update?)
	} else {
		None
	};
	Ok((update, resp == ResponseLine::Cruft))
}

fn read_line_from_mame(
	mame_stdout: &mut impl BufRead,
	mame_stderr: &mut impl BufRead,
	line: &mut String,
) -> Result<()> {
	line.clear();
	match mame_stdout.read_line(line) {
		Ok(0) => {
			let mame_stderr_text = read_text_from_reader(mame_stderr);
			Err(Error::EofFromMame(mame_stderr_text).into())
		}
		Ok(_) => Ok(()),
		Err(e) => Err(Error::ReadingFromMame(Box::new(e)).into()),
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

	fn mame_write_err(e: impl std::error::Error + Send + Sync + 'static) -> Error {
		Error::WritingToMame(Box::new(e))
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
			software_name,
		} => {
			assert!(software_name.is_none());
			format!("START {machine_name}").into()
		}
		MameCommand::Stop => "STOP".into(),
		MameCommand::Pause => "PAUSE".into(),
		MameCommand::Resume => "RESUME".into(),
		MameCommand::Ping => "PING".into(),
	}
}
