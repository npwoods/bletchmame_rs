use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::spawn;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Error;
use anyhow::Result;
use slint::invoke_from_event_loop;
use strum::EnumProperty;
use throttle::Throttle;

use crate::appcommand::AppCommand;
use crate::info::InfoDb;
use crate::prefs::PrefsPaths;
use crate::runtime::args::preflight_checks_public;
use crate::runtime::args::PreflightProblem;
use crate::status::Status;
use crate::status::Update;
use crate::threadlocalbubble::ThreadLocalBubble;

#[derive(Clone)]
pub struct AppState {
	pub info_db: Option<Rc<InfoDb>>,
	phase: Phase,
	shutting_down: bool,
	callback: CommandCallback,
}

#[derive(Clone, Debug)]
enum Phase {
	Inactive {
		message: Message,
		submessage: Option<String>,
		button: Option<Button>,
		issues: Rc<[Message]>,
	},
	InfoDbBuilding {
		job: Job<Result<Option<InfoDb>>>,
		cancelled: Arc<AtomicBool>,
		machine_description: Option<String>,
	},
	Active {
		status: Rc<Status>,
	},
	Shutdown,
}

type CommandCallback = Rc<dyn Fn(AppCommand) + 'static>;

#[derive(Debug)]
pub struct Report<'a> {
	pub message: &'a Message,
	pub submessage: Option<&'a str>,
	pub button: Option<Button>,
	pub issues: &'a [Message],
}

#[derive(Clone, Debug)]
pub struct Button {
	pub text: &'static str,
	pub command: AppCommand,
}

#[derive(Debug)]
struct Job<T>(Rc<RefCell<Option<JoinHandle<T>>>>);

#[derive(strum::Display, Clone, Debug, EnumProperty)]
pub enum Message {
	// blank message
	#[strum(to_string = "")]
	Blank,

	// progress messages
	#[strum(to_string = "Building MAME machine info database...", props(Spinning = "true"))]
	BuildingInfoDb,
	#[strum(to_string = "Resetting MAME...", props(Spinning = "true"))]
	MameResetting,

	// failure conditions
	#[strum(to_string = "BletchMAME requires additional configuration in order to properly interface with MAME")]
	InadequateMameSetup,
	#[strum(to_string = "Processing machine information from MAME was cancelled")]
	InfoDbBuildCancelled,
	#[strum(to_string = "Failure processing machine information from MAME")]
	InfoDbBuildFailure,

	// preflight problems
	#[strum(to_string = "No MAME executable path specified")]
	NoMameExecutablePath,
	#[strum(to_string = "No MAME executable found")]
	NoMameExecutable,
	#[strum(to_string = "MAME executable file is not executable")]
	MameExecutableIsNotExecutable,
	#[strum(to_string = "No valid plugins paths specified")]
	NoPluginsPaths,
	#[strum(to_string = "MAME boot.lua not found")]
	PluginsBootNotFound,
	#[strum(to_string = "BletchMAME worker_ui plugin not found")]
	WorkerUiPluginNotFound,
}

impl AppState {
	/// Creates an initial `AppState`
	pub fn new(callback: impl Fn(AppCommand) + 'static) -> Self {
		let callback = Rc::from(callback);
		Self {
			info_db: None,
			phase: Phase::Inactive {
				message: Message::Blank,
				submessage: None,
				button: None,
				issues: [].into(),
			},
			shutting_down: false,
			callback,
		}
	}

	/// Attempt to load a persisted InfoDB, or if unavailable trigger a rebuild
	pub fn infodb_load(&self, prefs_path: &Path, paths: &PrefsPaths, force_refresh: bool) -> Option<Self> {
		// try to load the InfoDb
		let info_db = paths
			.mame_executable
			.as_deref()
			.and_then(|mame_executable_path| InfoDb::load(prefs_path, mame_executable_path).ok())
			.map(Rc::new);

		// quick run of preflight
		let problems = preflight_checks_public(paths.mame_executable.as_deref(), &paths.plugins);

		// determine the new phase
		let phase = if !problems.is_empty() {
			let issues = problems.into_iter().map(Message::from).collect();
			Phase::Inactive {
				message: Message::InadequateMameSetup,
				submessage: None,
				button: None,
				issues,
			}
		} else if info_db.is_none() || force_refresh {
			let (job, cancelled) = spawn_infodb_build_thread(
				prefs_path,
				paths.mame_executable.as_ref().unwrap(),
				self.callback.clone(),
			);
			Phase::InfoDbBuilding {
				job,
				cancelled,
				machine_description: None,
			}
		} else {
			Phase::initial_active()
		};

		// and return
		let new_state = Self {
			info_db,
			phase,
			..self.clone()
		};
		Some(new_state)
	}

	pub fn infodb_build_progress(&self, machine_description: String) -> Option<Self> {
		let Phase::InfoDbBuilding { job, cancelled, .. } = &self.phase else {
			unreachable!()
		};

		let phase = Phase::InfoDbBuilding {
			job: job.clone(),
			cancelled: cancelled.clone(),
			machine_description: Some(machine_description),
		};
		let new_state = Self { phase, ..self.clone() };
		Some(new_state)
	}

	pub fn infodb_build_complete(&self) -> Option<Self> {
		self.internal_infodb_build_complete(false)
	}

	pub fn infodb_build_cancel(&self) -> Option<Self> {
		self.internal_infodb_build_complete(true)
	}

	fn internal_infodb_build_complete(&self, cancel: bool) -> Option<Self> {
		// we expect to be in the process of building, and to be able to "take" the job
		let Phase::InfoDbBuilding { job, cancelled, .. } = &self.phase else {
			unreachable!()
		};

		// if specified, cancel the build
		if cancel {
			cancelled.store(true, Ordering::Relaxed);
		}

		// join the job (which we expect to complete) and digest the result
		//
		// take note that cancelling is deviously involved; take note of the following:
		//   - we ignore the result from the job; there can be a race condition where the
		//     job actually yields something other than `Ok(None)`
		//   - we might have had an existing InfoDb; it should be used if available
		let result = job.join().unwrap();
		let result = if cancel { Ok(None) } else { result };
		let result = match (result, &self.info_db) {
			(Ok(Some(info_db)), _) => Ok(Rc::new(info_db)),
			(Ok(None), None) => Err((Message::InfoDbBuildCancelled, None)),
			(Ok(None), Some(old_info_db)) => Ok(old_info_db.clone()),
			(Err(e), _) => Err((Message::InfoDbBuildFailure, Some(e.to_string()))),
		};

		// get the InfoDb object and the phase
		let (info_db, phase) = match result {
			Ok(info_db) => (Some(info_db), Phase::initial_active()),
			Err((message, submessage)) => {
				let button = Button {
					text: "Retry",
					command: AppCommand::InfoDbBuildLoad { force_refresh: false },
				};
				let phase = Phase::Inactive {
					message,
					submessage,
					button: Some(button),
					issues: [].into(),
				};
				(None, phase)
			}
		};

		// and return the new state
		let new_state = Self {
			info_db,
			phase,
			..self.clone()
		};
		Some(new_state)
	}

	/// Apply a `worker_ui` status update
	pub fn status_update(&self, update: Update) -> Option<Self> {
		let status = Rc::new(self.status().unwrap().merge(update));
		let phase = Phase::Active { status };
		let new_state = Self { phase, ..self.clone() };
		Some(new_state)
	}

	/// The MAME session ended; return a new state
	pub fn session_ended(&self) -> Option<Self> {
		match &self.phase {
			Phase::Inactive { .. } => unreachable!(),
			Phase::InfoDbBuilding { .. } => None,
			Phase::Active { .. } => {
				// TODO - we should report errors; for now we're
				// just going to restart
				let phase = if self.shutting_down {
					Phase::Shutdown
				} else {
					Phase::initial_active()
				};
				let new_state = Self { phase, ..self.clone() };
				Some(new_state)
			}
			Phase::Shutdown => Some(self.clone()),
		}
	}

	pub fn shutdown(&self) -> Option<Self> {
		let phase = if let Phase::Inactive { .. } = &self.phase {
			Phase::Shutdown
		} else {
			self.phase.clone()
		};
		let new_state = Self {
			phase,
			shutting_down: true,
			..self.clone()
		};
		Some(new_state)
	}

	pub fn status(&self) -> Option<&'_ Status> {
		if let Phase::Active { status } = &self.phase {
			Some(status.as_ref())
		} else {
			None
		}
	}

	pub fn has_infodb_mismatch(&self) -> bool {
		if let Some(status) = self.status() {
			Option::zip(self.info_db.as_ref(), status.build.as_ref())
				.is_some_and(|(info_db, build)| info_db.build() != build)
		} else {
			false
		}
	}

	pub fn running_machine_description(&self) -> &'_ str {
		self.status()
			.and_then(|s| s.running.as_ref())
			.map(|r| {
				self.info_db
					.as_ref()
					.unwrap()
					.machines()
					.find(&r.machine_name)
					.unwrap()
					.name()
			})
			.unwrap_or_default()
	}

	pub fn report(&self) -> Option<Report<'_>> {
		match &self.phase {
			Phase::Inactive {
				message,
				submessage,
				button,
				issues,
			} => {
				let report = Report {
					message,
					submessage: submessage.as_deref(),
					button: button.clone(),
					issues,
				};
				Some(report)
			}

			Phase::InfoDbBuilding {
				machine_description, ..
			} => {
				let message = &Message::BuildingInfoDb;
				let button = Button {
					text: "Cancel",
					command: AppCommand::InfoDbBuildCancel,
				};
				let report = Report {
					message,
					submessage: machine_description.as_deref(),
					button: Some(button),
					issues: &[],
				};
				Some(report)
			}

			Phase::Active { status } => (!status.has_initialized).then(|| {
				let message = &Message::MameResetting;
				let button = Button {
					text: "Cancel",
					command: AppCommand::FileStop,
				};
				Report {
					message,
					submessage: None,
					button: Some(button),
					issues: &[],
				}
			}),

			Phase::Shutdown => {
				let report = Report {
					message: &Message::Blank,
					submessage: None,
					button: None,
					issues: &[],
				};
				Some(report)
			}
		}
	}

	pub fn is_shutdown(&self) -> bool {
		matches!(self.phase, Phase::Shutdown)
	}
}

impl Phase {
	pub fn initial_active() -> Self {
		let status = Rc::new(Status::default());
		Phase::Active { status }
	}
}

impl Message {
	pub fn spinning(&self) -> bool {
		match self.get_str("Spinning") {
			None => false,
			Some("true") => true,
			_ => unreachable!(),
		}
	}
}

impl From<PreflightProblem> for Message {
	fn from(value: PreflightProblem) -> Self {
		match value {
			PreflightProblem::NoMameExecutablePath => Message::NoMameExecutablePath,
			PreflightProblem::NoMameExecutable => Message::NoMameExecutable,
			PreflightProblem::MameExecutableIsNotExecutable => Message::MameExecutableIsNotExecutable,
			PreflightProblem::NoPluginsPaths => Message::NoPluginsPaths,
			PreflightProblem::PluginsBootNotFound => Message::PluginsBootNotFound,
			PreflightProblem::WorkerUiPluginNotFound => Message::WorkerUiPluginNotFound,
		}
	}
}

fn spawn_infodb_build_thread(
	prefs_path: &Path,
	mame_executable_path: &str,
	callback: CommandCallback,
) -> (Job<Result<Option<InfoDb>>>, Arc<AtomicBool>) {
	let prefs_path = prefs_path.to_path_buf();
	let mame_executable_path = mame_executable_path.to_string();
	let callback_bubble = ThreadLocalBubble::new(callback);
	let cancelled = Arc::new(AtomicBool::from(false));
	let job = {
		let cancelled = cancelled.clone();
		Job::new(move || infodb_build_thread_proc(&prefs_path, &mame_executable_path, callback_bubble, cancelled))
	};
	(job, cancelled)
}

fn infodb_build_thread_proc(
	prefs_path: &Path,
	mame_executable_path: &str,
	callback_bubble: ThreadLocalBubble<CommandCallback>,
	cancelled: Arc<AtomicBool>,
) -> Result<Option<InfoDb>> {
	// progress messages need to be throttled
	let mut throttle = Throttle::new(Duration::from_millis(100), 1);

	// lambda to invoke a command on the main event loop; there is some nontrivial stuff here
	// because of the need to put the callback in the "bubble" as well as to ensure that we
	// don't invoke the command if the user cancelled
	let cancelled_clone = cancelled.clone();
	let invoke_command = move |command| {
		let callback_bubble = callback_bubble.clone();
		let cancelled_clone = cancelled_clone.clone();
		invoke_from_event_loop(move || {
			if !cancelled_clone.load(Ordering::Relaxed) {
				(callback_bubble.unwrap())(command);
			}
		})
		.unwrap();
	};

	// prep a callback for progress
	let invoke_command_clone = invoke_command.clone();
	let callback = move |machine_description: &str| {
		// do we need to update
		if throttle.accept().is_ok() {
			let machine_description = machine_description.to_string();
			let command = AppCommand::InfoDbBuildProgress { machine_description };
			invoke_command_clone(command);
		}

		// have we cancelled?
		cancelled.load(Ordering::Relaxed)
	};

	// invoke MAME with `-listxml`
	let result = InfoDb::from_child_process(mame_executable_path, callback);

	// save the InfoDb (if we got one)
	if let Ok(Some(info_db)) = &result {
		let _ = info_db.save(prefs_path, mame_executable_path);
	}

	// signal that we're done
	invoke_command(AppCommand::InfoDbBuildComplete);

	// and return the result
	result
}

impl<T> Job<T>
where
	T: Send + 'static,
{
	pub fn new(f: impl FnOnce() -> T + Send + 'static) -> Self {
		let join_handle = spawn(f);
		Self(Rc::new(RefCell::new(Some(join_handle))))
	}

	pub fn join(&self) -> Result<T> {
		let join_handle = self.0.borrow_mut().take().ok_or_else(|| {
			let message = "Job::join() invoked multiple times";
			Error::msg(message)
		})?;
		let result = join_handle.join().map_err(|_| {
			let message = "JoinHandle::join() failed";
			Error::msg(message)
		})?;
		Ok(result)
	}
}

impl<T> Clone for Job<T> {
	fn clone(&self) -> Self {
		Self(Rc::clone(&self.0))
	}
}
