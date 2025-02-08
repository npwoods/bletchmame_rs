use std::borrow::Cow;
use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
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
use throttle::Throttle;

use crate::appcommand::AppCommand;
use crate::dialogs::file::PathType;
use crate::info::InfoDb;
use crate::prefs::PrefsPaths;
use crate::runtime::args::preflight_checks;
use crate::runtime::args::MameArgumentsSource;
use crate::runtime::args::PreflightProblem;
use crate::runtime::session::MameSession;
use crate::runtime::MameCommand;
use crate::runtime::MameEvent;
use crate::runtime::MameStderr;
use crate::runtime::MameWindowing;
use crate::status::Status;
use crate::status::Update;
use crate::status::UpdateXmlProblem;
use crate::status::ValidationError;
use crate::threadlocalbubble::ThreadLocalBubble;

#[derive(Clone)]
pub struct AppState {
	info_db: Option<Rc<InfoDb>>,
	paths: Rc<PrefsPaths>,
	info_db_build: Option<InfoDbBuild>,
	session: Option<Session>,
	failure: Option<Rc<Failure>>,
	pending_restart: bool,
	pending_shutdown: bool,
	fixed: Rc<Fixed>,
}

#[derive(Clone)]
struct InfoDbBuild {
	job: Job<Result<Option<InfoDb>>>,
	cancelled: Arc<AtomicBool>,
	machine_description: Option<String>,
}

#[derive(Clone)]
struct Session {
	mame_session: Rc<RefCell<Option<MameSession>>>,
	status: Option<Rc<Status>>,
}

#[derive(Debug)]
enum Failure {
	Preflight(Vec<PreflightProblem>),
	SessionError(Error),
	InvalidStatusUpdate(Vec<UpdateXmlProblem>),
	InfoDbBuild(Error),
	InfoDbBuildCancelled,
}

struct Fixed {
	prefs_path: PathBuf,
	mame_windowing: MameWindowing,
	mame_stderr: MameStderr,
	callback: CommandCallback,
}

type CommandCallback = Rc<dyn Fn(AppCommand) + 'static>;

#[derive(Default, Debug)]
pub struct Report<'a> {
	pub message: Cow<'a, str>,
	pub submessage: Option<Cow<'a, str>>,
	pub button: Option<Button>,
	pub is_spinning: bool,
	pub issues: Vec<Issue>,
}

#[derive(Clone, Debug)]
pub struct Button {
	pub text: Cow<'static, str>,
	pub command: AppCommand,
}

#[derive(Clone, Debug)]
pub struct Issue {
	pub text: Cow<'static, str>,
	pub button: Option<Button>,
}

#[derive(Debug)]
struct Job<T>(Rc<RefCell<Option<JoinHandle<T>>>>);

impl AppState {
	/// Creates an initial `AppState`
	pub fn new(
		prefs_path: PathBuf,
		paths: Rc<PrefsPaths>,
		mame_windowing: MameWindowing,
		mame_stderr: MameStderr,
		callback: impl Fn(AppCommand) + 'static,
	) -> Self {
		let callback = Rc::from(callback);
		let fixed = Fixed {
			prefs_path,
			mame_windowing,
			mame_stderr,
			callback,
		};
		let fixed = Rc::new(fixed);
		Self {
			info_db: None,
			paths,
			info_db_build: None,
			session: None,
			failure: None,
			pending_restart: false,
			pending_shutdown: false,
			fixed,
		}
	}

	/// Creates a "bogus" AppState that should never be used
	pub fn bogus() -> Self {
		Self::new(
			"".into(),
			Rc::new(PrefsPaths::default()),
			MameWindowing::Attached("".into()),
			MameStderr::Capture,
			|_| unreachable!(),
		)
	}

	pub fn update_paths(&self, paths: &Rc<PrefsPaths>) -> Option<Self> {
		if self.paths.as_ref() == paths.as_ref() {
			return None;
		}

		let state = Self {
			info_db: None,
			paths: paths.clone(),
			..self.clone()
		};

		let state = if self.paths.as_ref().mame_executable != paths.as_ref().mame_executable {
			let state = state.infodb_load();
			state.reset(true, state.info_db.is_none()).unwrap_or(state)
		} else {
			state
		};

		Some(state)
	}

	/// Issues a command to MAME
	pub fn issue_command(&self, command: MameCommand<'_>) {
		let session = self.session.as_ref().unwrap();
		session.mame_session.borrow().as_ref().unwrap().issue_command(command);
	}

	/// Do we have an active session, and we have an empty queue?
	pub fn is_running_with_queue_empty(&self) -> bool {
		self.session
			.as_ref()
			.map(|s| s.status.is_some() && !s.mame_session.borrow().as_ref().unwrap().has_pending_commands())
			.unwrap_or_default()
	}

	/// Attempt to load a persisted InfoDB
	pub fn infodb_load(&self) -> Self {
		// try to load the InfoDb
		let info_db = self
			.paths
			.as_ref()
			.mame_executable
			.as_deref()
			.and_then(|mame_executable_path| InfoDb::load(&self.fixed.prefs_path, mame_executable_path).ok())
			.map(Rc::new);

		Self {
			info_db,
			..self.clone()
		}
	}

	pub fn reset(&self, mut reset_session: bool, rebuild_info_db: bool) -> Option<Self> {
		// sanity checks
		assert!(!rebuild_info_db || self.info_db_build.is_none());
		if !rebuild_info_db && !reset_session {
			return None;
		}

		// quick run of preflight
		let mame_executable_path = self.paths.mame_executable.as_deref();
		let preflight_problems = preflight_checks(mame_executable_path, &self.paths.plugins);

		// start an InfoDb build; if we can
		let rebuild_info_db = rebuild_info_db
			&& !preflight_problems
				.iter()
				.any(|x| x.problem_type() == Some(PathType::MameExecutable));
		let info_db_build = rebuild_info_db.then(|| {
			let prefs_path = &self.fixed.prefs_path;
			let mame_executable_path = mame_executable_path.unwrap();
			let callback = self.fixed.callback.clone();
			let (job, cancelled) = spawn_infodb_build_thread(prefs_path, mame_executable_path, callback);
			InfoDbBuild {
				job,
				cancelled,
				machine_description: None,
			}
		});

		// do we need to defer starting a MAME session?
		let mut pending_restart = false;
		if let Some(session) = self.session.as_ref() {
			if reset_session && preflight_problems.is_empty() {
				session
					.mame_session
					.borrow()
					.as_ref()
					.unwrap()
					.issue_command(MameCommand::Exit);
				pending_restart = true;
				reset_session = false;
			}
		}

		// start a MAME session; if we can
		let reset_session = reset_session && preflight_problems.is_empty();
		let session = reset_session.then(|| {
			let callback_bubble = ThreadLocalBubble::new(self.fixed.callback.clone());
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
			let mame_args = MameArgumentsSource::new(self.paths.as_ref(), &self.fixed.mame_windowing).into();
			let mame_session = MameSession::new(mame_args, event_callback, self.fixed.mame_stderr);
			let mame_session = Rc::new(RefCell::new(Some(mame_session)));
			Session {
				mame_session,
				status: None,
			}
		});

		// format the preflight failures (if present)
		let failure = (!preflight_problems.is_empty())
			.then(|| Rc::new(Failure::Preflight(preflight_problems.into_iter().collect())));

		// assemble and return the new state
		let info_db_build = info_db_build.or_else(|| self.info_db_build.clone());
		let session = session.or_else(|| self.session.clone());
		let new_state = Self {
			failure,
			info_db_build,
			session,
			pending_restart,
			..self.clone()
		};
		Some(new_state)
	}

	pub fn infodb_build_progress(&self, machine_description: String) -> Self {
		let info_db_build = InfoDbBuild {
			machine_description: Some(machine_description),
			..self.info_db_build.as_ref().unwrap().clone()
		};
		let info_db_build = Some(info_db_build);

		Self {
			info_db_build,
			..self.clone()
		}
	}

	pub fn infodb_build_complete(&self) -> Self {
		self.internal_infodb_build_complete(false)
	}

	pub fn infodb_build_cancel(&self) -> Self {
		self.internal_infodb_build_complete(true)
	}

	fn internal_infodb_build_complete(&self, cancel: bool) -> Self {
		// we expect to be in the process of building, and to be able to "take" the job
		let info_db_build = self.info_db_build.as_ref().unwrap();

		// if specified, cancel the build
		if cancel {
			info_db_build.cancelled.store(true, Ordering::Relaxed);
		}

		// join the job (which we expect to complete) and digest the result
		//
		// take note that cancelling is deviously involved; take note of the following:
		//   - we ignore the result from the job; there can be a race condition where the
		//     job actually yields something other than `Ok(None)`
		//   - we might have had an existing InfoDb; it should be used if available
		let result = info_db_build.job.join().unwrap();
		let result = if cancel { Ok(None) } else { result };

		// get the InfoDb object and the phase
		let (info_db, failure) = match (result, self.info_db.as_ref()) {
			(Ok(Some(info_db)), _) => (Some(Rc::new(info_db)), None),
			(Ok(None), None) => (None, Some(Failure::InfoDbBuildCancelled)),
			(Ok(None), Some(old_info_db)) => (Some(old_info_db.clone()), None),
			(Err(e), old_info_db) => (old_info_db.cloned(), Some(Failure::InfoDbBuild(e))),
		};

		// and return the new state
		let failure = failure.map(Rc::new).or_else(|| self.failure.clone());
		Self {
			info_db,
			info_db_build: None,
			failure,
			..self.clone()
		}
	}

	/// Apply a `worker_ui` status update
	pub fn status_update(&self, update: Update) -> Option<Self> {
		// validate the status update
		if let Err(e) = update.validate(self.info_db.as_ref().unwrap()) {
			return match e {
				ValidationError::VersionMismatch(_, _) => self.reset(true, true),
				ValidationError::Invalid(errors) => {
					let failure = Some(Rc::new(Failure::InvalidStatusUpdate(errors)));
					let new_status = Self {
						failure,
						..self.clone()
					};
					Some(new_status)
				}
			};
		}

		// merge the new status
		let new_status = self
			.session
			.as_ref()
			.unwrap()
			.status
			.as_deref()
			.map(Cow::Borrowed)
			.unwrap_or_else(|| Cow::Owned(Status::default()))
			.merge(update);

		// update the session
		let session = Session {
			status: Some(Rc::new(new_status)),
			..self.session.as_ref().unwrap().clone()
		};

		// and return the new state
		let new_state = Self {
			session: Some(session),
			..self.clone()
		};
		Some(new_state)
	}

	/// The MAME session ended; return a new state
	pub fn session_ended(&self) -> Self {
		// join the thread and get the result
		let Some(session) = self.session.as_ref() else {
			unreachable!();
		};
		let result = session.mame_session.borrow_mut().take().unwrap().shutdown();

		// if we failed, we have to report the error
		let failure = if let Err(e) = result {
			Some(Rc::new(Failure::SessionError(e)))
		} else {
			None
		};

		// create the new state
		let pending_restart = self.pending_restart && failure.is_none();
		let mut new_state = Self {
			session: None,
			pending_restart,
			failure,
			..self.clone()
		};

		// if there is a pending restart, kick it off
		if new_state.pending_restart {
			new_state = new_state.reset(true, false).unwrap_or(new_state);
		}

		// and return
		new_state
	}

	pub fn shutdown(&self) -> Option<Self> {
		(!self.pending_shutdown).then(|| {
			if self.session.is_some() {
				self.issue_command(MameCommand::Exit);
			}
			Self {
				pending_shutdown: true,
				..self.clone()
			}
		})
	}

	pub fn info_db(&self) -> Option<&'_ Rc<InfoDb>> {
		self.info_db.as_ref()
	}

	pub fn status(&self) -> Option<&'_ Status> {
		self.session.as_ref().and_then(|x| x.status.as_deref())
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
		#[derive(Debug)]
		enum ReportType<'a> {
			InfoDbBuild(Option<&'a str>),
			Resetting,
			ShuttingDown,
			PreflightFailure(&'a [PreflightProblem]),
			SessionError(&'a Error),
			InvalidStatusUpdate(&'a [UpdateXmlProblem]),
			InfoDbBuildFailure(Option<&'a Error>),
		}

		// upfront logic to determine the type of report presented, if any; keep
		// this logic distinct from the mechanics of displaying the report
		let is_starting_up = self.session.as_ref().is_some_and(|x| x.status.is_none());
		let report_type = match (
			self.info_db_build.as_ref(),
			self.failure.as_deref(),
			is_starting_up,
			self.pending_shutdown,
		) {
			(Some(info_db_build), _, _, _) => {
				Some(ReportType::InfoDbBuild(info_db_build.machine_description.as_deref()))
			}
			(None, _, _, true) => Some(ReportType::ShuttingDown),
			(None, _, true, false) => Some(ReportType::Resetting),
			(None, Some(Failure::Preflight(preflight_problems)), false, false) => {
				Some(ReportType::PreflightFailure(preflight_problems.as_slice()))
			}
			(None, Some(Failure::SessionError(e)), false, false) => Some(ReportType::SessionError(e)),
			(None, Some(Failure::InvalidStatusUpdate(e)), false, false) => {
				Some(ReportType::InvalidStatusUpdate(e.as_slice()))
			}
			(None, Some(Failure::InfoDbBuild(e)), false, false) => Some(ReportType::InfoDbBuildFailure(Some(e))),
			(None, Some(Failure::InfoDbBuildCancelled), false, false) => Some(ReportType::InfoDbBuildFailure(None)),
			(None, None, false, false) => None,
		};

		report_type.map(|report_type| match report_type {
			ReportType::InfoDbBuild(machine_description) => {
				let message = Cow::Borrowed("Building MAME machine info database...");
				let submessage = machine_description.map(Cow::Borrowed).unwrap_or_default();
				let button = Button {
					text: "Cancel".into(),
					command: AppCommand::InfoDbBuildCancel,
				};
				Report {
					message,
					submessage: Some(submessage),
					button: Some(button),
					is_spinning: true,
					..Default::default()
				}
			}
			ReportType::Resetting => Report {
				message: "Resetting MAME...".into(),
				is_spinning: true,
				..Default::default()
			},
			ReportType::ShuttingDown => Report {
				message: "MAME is shutting down...".into(),
				is_spinning: true,
				..Default::default()
			},
			ReportType::PreflightFailure(preflight_problems) => {
				let message = Cow::Borrowed(
					"BletchMAME requires additional configuration in order to properly interface with MAME",
				);
				let issues = preflight_problems
					.iter()
					.map(|problem| {
						let text = problem.to_string().into();
						let button = problem.problem_type().map(|path_type| {
							let text = Cow::Owned(format!("Choose {path_type}"));
							let command = AppCommand::ChoosePath(path_type);
							Button { text, command }
						});
						Issue { text, button }
					})
					.collect();
				Report {
					message,
					issues,
					..Default::default()
				}
			}
			ReportType::SessionError(error) => Report {
				message: "MAME has errored".into(),
				submessage: Some(format!("{error}").into()),
				..Default::default()
			},
			ReportType::InvalidStatusUpdate(errors) => {
				let issues = errors
					.iter()
					.map(|e| Issue {
						text: format!("{e}").into(),
						button: None,
					})
					.collect();
				Report {
					message: "Status update from MAME is incorrect".into(),
					issues,
					..Default::default()
				}
			}
			ReportType::InfoDbBuildFailure(error) => {
				let message = if error.is_some() {
					"Failure processing machine information from MAME"
				} else {
					"Processing machine information from MAME was cancelled"
				};
				let submessage = error.map(|e| Cow::Owned(e.to_string()));
				let button = Button {
					text: "Retry".into(),
					command: AppCommand::HelpRefreshInfoDb,
				};
				Report {
					message: Cow::Borrowed(message),
					submessage,
					button: Some(button),
					..Default::default()
				}
			}
		})
	}

	pub fn is_building_infodb(&self) -> bool {
		self.info_db_build.is_some()
	}

	pub fn is_shutdown(&self) -> bool {
		self.pending_shutdown && self.info_db_build.is_none() && self.session.is_none()
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
