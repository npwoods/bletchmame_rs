use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::Error;
use anyhow::Result;
use more_asserts::assert_gt;
use slint::invoke_from_event_loop;
use smol_str::SmolStr;
use smol_str::ToSmolStr;
use smol_str::format_smolstr;
use throttle::Throttle;
use tracing::debug;

use crate::action::Action;
use crate::canceller::Canceller;
use crate::console::Console;
use crate::info::InfoDb;
use crate::job::Job;
use crate::prefs::Preferences;
use crate::prefs::PreflightProblem;
use crate::prefs::PrefsVideo;
use crate::runtime::MameStartArgs;
use crate::runtime::MameStderr;
use crate::runtime::MameWindowing;
use crate::runtime::args::MameArguments;
use crate::runtime::command::MameCommand;
use crate::runtime::session::spawn_mame_session_thread;
use crate::status::Status;
use crate::status::Update;
use crate::status::UpdateXmlProblem;
use crate::status::ValidationError;
use crate::threadlocalbubble::ThreadLocalBubble;
use crate::version::MameVersion;

use crate::runtime::session::Error as SessionError;
use crate::runtime::session::Result as SessionResult;

pub struct AppState {
	pub preferences: Preferences,
	info_db_build: Option<InfoDbBuild>,
	live: Option<Live>,
	failure: Option<Failure>,
	last_save_state: Option<Box<str>>,
	video_override: Option<PrefsVideo>,
	fixed: Fixed,
}

/// Represents the state of an InfoDb build (-listxml) job
struct InfoDbBuild {
	job: Job<Result<Option<InfoDb>>>,
	canceller: Canceller,
	machine_description: Option<String>,
}

/// Represents so-called "live" state; we have an InfoDb and maybe a build
struct Live {
	info_db: Rc<InfoDb>,
	session: Option<Session>,
}

/// Represents a session and associated communication
struct Session {
	job: Job<SessionResult<()>>,
	command_sender: Option<Arc<Sender<MameCommand>>>,
	status: Option<Rc<Status>>,
	pending_status: Option<Rc<Status>>,
	expecting: Option<Expecting>,
	post_session_end: Option<PostSessionEnd>,
}

#[derive(Debug)]
enum PostSessionEnd {
	Shutdown,
	Restart { command: Option<MameCommand> },
}

#[derive(Debug)]
enum Failure {
	Preflight(Box<[PreflightProblem]>),
	SessionError(SessionError),
	StatusValidationProblem(ValidationError),
	InfoDbBuild(Error),
	InfoDbBuildCancelled,
}

#[derive(Debug, PartialEq, Eq)]
enum Expecting {
	Start,
	Stop,
}

struct Fixed {
	prefs_path: PathBuf,
	mame_stderr: MameStderr,
	mame_windowing: MameWindowing,
	console: Arc<Mutex<Option<Console>>>,
	callback: CommandCallback,
}

type CommandCallback = Rc<dyn Fn(Action) + 'static>;

#[derive(Default, Debug)]
pub struct Report {
	pub message: SmolStr,
	pub submessage: Option<SmolStr>,
	pub mame_stderr_output: Option<SmolStr>,
	pub mame_exit_code: Option<i32>,
	pub button: Option<Button>,
	pub is_spinning: bool,
	pub issues: Vec<Issue>,
}

#[derive(Clone, Debug)]
pub struct Button {
	pub text: SmolStr,
	pub command: Action,
}

#[derive(Clone, Debug)]
pub struct Issue {
	pub text: SmolStr,
	pub button: Option<Button>,
}

impl AppState {
	/// Creates an initial `AppState`
	pub fn new(
		prefs_path: PathBuf,
		mame_stderr: MameStderr,
		mame_windowing: MameWindowing,
		callback: impl Fn(Action) + 'static,
	) -> Self {
		let console = Arc::new(Mutex::new(None));
		let callback = Rc::from(callback);
		let fixed = Fixed {
			prefs_path,
			mame_stderr,
			mame_windowing,
			console,
			callback,
		};
		Self {
			preferences: Preferences::default(),
			info_db_build: None,
			live: None,
			failure: None,
			last_save_state: None,
			video_override: None,
			fixed,
		}
	}

	/// Creates a "bogus" AppState that should never be used
	pub fn bogus() -> Self {
		Self::new(
			"".into(),
			MameStderr::Capture,
			MameWindowing::Windowed,
			|_| unreachable!(),
		)
	}

	fn make_mame_args(&mut self) -> std::result::Result<MameArguments, ()> {
		// clear out any failures
		self.failure = None;

		// create MAME arguments
		let mame_args_result = MameArguments::new(
			&self.preferences,
			self.video_override.as_ref(),
			&self.fixed.mame_windowing,
			false,
		);

		// if we failed, report it
		mame_args_result.map_err(|e| {
			// failed preflight? report the problems
			assert_gt!(e.preflight_problems.len(), 0);
			self.failure = Some(Failure::Preflight(e.preflight_problems));
		})
	}

	pub fn activate(&mut self) -> bool {
		// if we already have a session (in any form, we're already active) or if we're shutting down, don't proceed
		if self.live.as_ref().is_some_and(|live| live.session.is_some()) {
			return false;
		}

		// get or load the InfoDb
		let mame_args_result = self.make_mame_args();
		let info_db = self.info_db().cloned().or_else(|| {
			let mame_executable_path = mame_args_result.as_ref().as_ref().ok()?.program.as_str();
			let info_db = InfoDb::load(&self.fixed.prefs_path, mame_executable_path).ok()?;
			Some(Rc::new(info_db))
		});

		if let Some(info_db) = info_db {
			let session = mame_args_result.map(|mame_args| self.start_session(mame_args)).ok();
			self.live = Some(Live { info_db, session });
			true
		} else {
			// we don't have InfoDb; force a rebuild
			self.infodb_rebuild()
		}
	}

	fn start_session(&self, mame_args: MameArguments) -> Session {
		let (job, command_sender) = spawn_mame_session_thread(
			mame_args,
			self.fixed.mame_stderr,
			self.fixed.console.clone(),
			self.fixed.callback.clone(),
		);
		let command_sender = Some(Arc::new(command_sender));
		Session {
			job,
			command_sender,
			status: None,
			expecting: None,
			pending_status: None,
			post_session_end: None,
		}
	}

	pub fn infodb_rebuild(&mut self) -> bool {
		if self.info_db_build.is_some() {
			return false;
		}

		// access the MAME executable path (or preflight errors if we don't have them)
		if let Ok(mame_args) = self.make_mame_args() {
			let mame_executable_path = mame_args.program.as_str();
			let prefs_path = &self.fixed.prefs_path;
			let callback = self.fixed.callback.clone();
			let (job, canceller) = spawn_infodb_build_thread(prefs_path, mame_executable_path, callback);
			let info_db_build = InfoDbBuild {
				job,
				canceller,
				machine_description: None,
			};
			self.info_db_build = Some(info_db_build);
		};
		true
	}

	pub fn reset(&mut self) -> bool {
		// if a session is live, set it to stop and restart
		if let Some(session) = self.live.as_mut().and_then(|live| live.session.as_mut()) {
			session.command_sender = None;
			session.post_session_end = Some(PostSessionEnd::Restart { command: None });
		}

		// attempt to reactivate and return
		self.activate();
		true
	}

	pub fn start(&mut self, start_args: MameStartArgs) -> bool {
		// build the command that will ultimately be issued to MAME/worker_ui
		let command = MameCommand::start(&start_args);

		// access the live session (which had better be present)
		let session = self.live.as_mut().unwrap().session.as_mut().unwrap();

		// we're expecting to run
		session.expecting = Some(Expecting::Start);

		// is the session live and ready?
		if let Some(command_sender) = &session.command_sender
			&& self.video_override == start_args.video
		{
			// if so, issue the command to start
			command_sender.send(command).unwrap();
		} else {
			// if not, set us up to restart with the new command
			let command = Some(command);
			self.video_override = start_args.video;
			session.command_sender = None;
			session.post_session_end = Some(PostSessionEnd::Restart { command });
		}
		true
	}

	pub fn stop(&mut self) -> bool {
		// access the live session (which had better be present)
		let session = self.live.as_mut().unwrap().session.as_mut().unwrap();

		let changed = session.expecting != Some(Expecting::Stop);
		session.expecting = Some(Expecting::Stop);
		self.issue_command(MameCommand::stop());
		changed
	}

	/// Issues a command to MAME
	pub fn issue_command(&self, command: MameCommand) {
		let session = self.live.as_ref().unwrap().session.as_ref().unwrap();
		if let Some(command_sender) = session.command_sender.as_deref() {
			command_sender.send(command).unwrap();
		}
	}

	pub fn infodb_build_progress(&mut self, machine_description: String) -> bool {
		self.info_db_build.as_mut().unwrap().machine_description = Some(machine_description);
		true
	}

	pub fn infodb_build_complete(&mut self) -> bool {
		self.internal_infodb_build_complete(false)
	}

	pub fn infodb_build_cancel(&mut self) -> bool {
		self.internal_infodb_build_complete(true)
	}

	fn internal_infodb_build_complete(&mut self, cancel: bool) -> bool {
		// we expect to be in the process of building, and to be able to "take" the job
		let info_db_build = self.info_db_build.as_ref().unwrap();

		// if specified, cancel the build
		if cancel {
			info_db_build.canceller.cancel();
		}

		// join the job (which we expect to complete) and digest the result
		//
		// take note that when we cancel, we ignore the result from the job; there
		// can be a race condition where the job actually yields something other than `Ok(None)`
		let result = info_db_build.job.join().unwrap();
		let result = if cancel { Ok(None) } else { result };

		// this next bit is pretty involved
		match result {
			// the rebuild succeeded
			Ok(Some(info_db)) => {
				// put the info_db into the "live" (creating one if we have)
				let info_db = Rc::new(info_db);
				if let Some(live) = self.live.as_mut() {
					live.info_db = info_db;
				} else {
					let live = Live { info_db, session: None };
					self.live = Some(live);
				}

				let live = self.live.as_mut().unwrap();
				let info_db = live.info_db.as_ref();
				if let Some(session) = live.session.as_mut() {
					// we do have a session; we need to validate and apply any pending status update
					let (status, pending_status, result) = validate_and_update_status(
						session.status.as_ref(),
						session.pending_status.as_ref(),
						None,
						info_db,
					);

					if let Err(e) = result {
						session.command_sender = None;
						self.failure = Some(Failure::StatusValidationProblem(e));
					} else {
						self.failure = None;
					}

					session.status = status;
					session.pending_status = pending_status;
				} else {
					// no session; create a new one
					if let Ok(mame_args) = self.make_mame_args() {
						let session = self.start_session(mame_args);
						self.live.as_mut().unwrap().session = Some(session);
					}
				};
			}

			// the user cancelled; present an error if we're not live
			Ok(None) => {
				if self.live.is_none() {
					self.failure = Some(Failure::InfoDbBuildCancelled);
				}
			}

			// an unexpected error occurred; shut down the live session (if any) and report the error
			Err(e) => {
				let session = self.live.as_mut().and_then(|live| live.session.as_mut());
				if let Some(session) = session {
					session.command_sender = None;
				}
				self.failure = Some(Failure::InfoDbBuild(e));
			}
		};

		// and return
		self.info_db_build = None;
		true
	}

	/// Apply a `worker_ui` status update
	pub fn status_update(&mut self, update: Update) -> bool {
		let live = self.live.as_mut().unwrap();
		let session = live.session.as_mut().unwrap();

		// ignore status updates when we're shutting down
		if session.command_sender.is_none() {
			return false;
		}

		// validate the status update
		let (new_status, new_pending_status, result) = validate_and_update_status(
			session.status.as_ref(),
			session.pending_status.as_ref(),
			Some(update),
			&live.info_db,
		);

		// respond to the results (do we report a failure?  force an info_db rebuild?)
		let (failure, rebuild_info_db) = match result {
			Ok(()) => (None, false),
			Err(ValidationError::VersionMismatch(_, _)) => (None, self.info_db_build.is_none()),
			Err(e) => (Some(Failure::StatusValidationProblem(e)), false),
		};

		// and munge this into the new state
		session.status = new_status;
		session.pending_status = new_pending_status;
		if let Some(failure) = failure {
			self.failure = Some(failure);
		}

		// update expectations
		if let Some(session) = self.live.as_mut().and_then(|l| l.session.as_mut()) {
			let is_running = session.status.as_deref().is_some_and(|s| s.running.is_some());
			if session.expecting == Some(Expecting::Start) && is_running
				|| session.expecting == Some(Expecting::Stop) && !is_running
			{
				session.expecting = None;
			}
		}

		// kick off an InfoDb rebuild if appropriate
		if rebuild_info_db {
			self.infodb_rebuild();
		}
		true
	}

	/// The MAME session ended; return a new state
	pub fn session_ended(&mut self) -> ControlFlow<()> {
		// access the "live" and the session
		let live = self.live.as_mut().unwrap();
		let session = live.session.as_mut().unwrap();

		// join the thread and get the result
		let result = session.job.join().unwrap();

		// if we failed, we have to report the error
		let failure = result.err().map(Failure::SessionError);

		// identify activities that need to happen after the session ends
		let (reactivate, command, result) = match session.post_session_end.take() {
			None => (false, None, ControlFlow::Continue(())),
			Some(PostSessionEnd::Restart { command }) => (true, command, ControlFlow::Continue(())),
			Some(PostSessionEnd::Shutdown) => (false, None, ControlFlow::Break(())),
		};

		// clear out the session
		if let Some(live) = self.live.as_mut() {
			live.session = None;
		}

		// identify failures
		self.failure = failure;

		// do we need to reactivate?
		if reactivate {
			self.activate();
		}

		// do we need to issue a command?
		if let Some(command) = command {
			// we do intend to issue a command, but verify that reactivation succeeded in creating one
			let session = self.live.as_mut().and_then(|live| live.session.as_mut());
			let has_session = session.is_some();
			if let Some(session) = session {
				// if we have a command, its because we're expecting an emulation to start
				session.expecting = Some(Expecting::Start);
			}
			if has_session {
				self.issue_command(command);
			}
		}

		// and we're done!
		result
	}

	pub fn shutdown(&mut self) -> ControlFlow<()> {
		let session = self.live.as_mut().and_then(|live| live.session.as_mut());
		if let Some(session) = session {
			session.command_sender = None;
			session.post_session_end = Some(PostSessionEnd::Shutdown);
			ControlFlow::Continue(())
		} else {
			ControlFlow::Break(())
		}
	}

	pub fn info_db(&self) -> Option<&'_ Rc<InfoDb>> {
		self.live.as_ref().map(|live| &live.info_db)
	}

	pub fn status(&self) -> Option<&'_ Status> {
		self.live
			.as_ref()
			.and_then(|live| live.session.as_ref())
			.and_then(|session| session.status.as_deref())
	}

	pub fn running_machine_description(&self) -> &'_ str {
		self.live
			.as_ref()
			.and_then(|live| {
				live.session
					.as_ref()
					.and_then(|session| session.status.as_deref())
					.and_then(|status| status.running.as_ref())
					.map(|running| live.info_db.machines().find(&running.machine_name).unwrap().name())
			})
			.unwrap_or_default()
	}

	pub fn report(&self) -> Option<Report> {
		#[derive(Debug)]
		enum ReportType<'a> {
			InfoDbBuild(Option<&'a str>),
			Resetting,
			Starting,
			Stopping,
			ShuttingDown,
			PreflightFailure(&'a [PreflightProblem]),
			SessionError(&'a SessionError),
			InvalidStatusUpdate(&'a [UpdateXmlProblem]),
			InfoDbBuildFailure(Option<&'a Error>),
			InfoDbStatusMismatch(&'a MameVersion, &'a MameVersion),
		}

		// upfront logic to determine the type of report presented, if any; keep
		// this logic distinct from the mechanics of displaying the report
		let session = self.live.as_ref().and_then(|live| live.session.as_ref());
		let expecting = session.and_then(|session| session.expecting.as_ref());
		let is_starting_up = session.is_some_and(|session| session.status.is_none());
		let is_session_stopping = session.is_some_and(|session| session.command_sender.is_none());
		let is_shutting_down =
			session.is_some_and(|session| matches!(session.post_session_end, Some(PostSessionEnd::Shutdown)));
		debug!(info_db_build=?self.info_db_build.as_ref().map(|_| "..."), ?expecting, failure=?self.failure.as_ref(), ?is_starting_up, ?is_shutting_down, "AppState::report()");
		let report_type = match (
			self.info_db_build.as_ref(),
			expecting,
			self.failure.as_ref(),
			is_starting_up,
			is_shutting_down,
		) {
			(Some(info_db_build), _, _, _, _) => {
				Some(ReportType::InfoDbBuild(info_db_build.machine_description.as_deref()))
			}
			(None, Some(Expecting::Start), None, _, _) => Some(ReportType::Starting),
			(None, Some(Expecting::Stop), None, _, _) => Some(ReportType::Stopping),
			(None, _, _, _, true) => Some(ReportType::ShuttingDown),
			(None, _, _, true, false) => Some(ReportType::Resetting),
			(None, _, Some(Failure::Preflight(preflight_problems)), false, false) => {
				Some(ReportType::PreflightFailure(preflight_problems.as_ref()))
			}
			(None, _, Some(Failure::SessionError(e)), false, false) => Some(ReportType::SessionError(e)),
			(None, _, Some(Failure::StatusValidationProblem(ValidationError::Invalid(e))), false, false) => {
				Some(ReportType::InvalidStatusUpdate(e.as_slice()))
			}
			(
				None,
				_,
				Some(Failure::StatusValidationProblem(ValidationError::VersionMismatch(status_build, infodb_build))),
				false,
				false,
			) => Some(ReportType::InfoDbStatusMismatch(status_build, infodb_build)),
			(None, _, Some(Failure::InfoDbBuild(e)), false, false) => Some(ReportType::InfoDbBuildFailure(Some(e))),
			(None, _, Some(Failure::InfoDbBuildCancelled), false, false) => Some(ReportType::InfoDbBuildFailure(None)),
			(None, None, None, false, false) => None,
		};

		report_type.map(|report_type| match report_type {
			ReportType::InfoDbBuild(machine_description) => {
				let message = "Building MAME machine info database...".into();
				let submessage = machine_description.map(|x| x.into()).unwrap_or_default();
				let button = Button {
					text: "Cancel".into(),
					command: Action::InfoDbBuildCancel,
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
			ReportType::Starting => {
				let submessage = match (is_session_stopping, is_starting_up) {
					(true, _) => "MAME needs to be reset to run this emulation",
					(false, true) => "MAME is reinitializing",
					(false, false) => "Waiting for emulation startup to be complete"
				};
				Report {
					message: "Starting emulation...".into(),
					submessage: Some(submessage.into()),
					is_spinning: true,
					..Default::default()
				}
			},
			ReportType::Stopping => Report {
				message: "Stopping emulation...".into(),
				is_spinning: true,
				..Default::default()
			},
			ReportType::ShuttingDown => Report {
				message: "MAME is shutting down...".into(),
				is_spinning: true,
				..Default::default()
			},
			ReportType::PreflightFailure(preflight_problems) => {
				let message = "BletchMAME requires additional configuration in order to properly interface with MAME".into();

				let issues = preflight_problems
					.iter()
					.map(|problem| {
						let text = problem.to_smolstr();
						let button = problem.problem_type().map(|path_type| {
							let text = format_smolstr!("Choose {path_type}");
							let command = Action::SettingsPaths(Some(path_type));
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
			ReportType::SessionError(error) => {
				let button = Button {
					text: "Continue".into(),
					command: Action::ReactivateMame
				};
				Report {
					message: "MAME has errored and shut down".into(),
					submessage: Some(format!("{error}").into()),
					mame_stderr_output: error.mame_stderr_text.clone(),
					mame_exit_code: error.exit_code,
					button: Some(button),
					..Default::default()
				}
			}
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
				let message = message.into();
				let submessage = error.map(|e| e.to_smolstr());
				let button = Button {
					text: "Retry".into(),
					command: Action::HelpRefreshInfoDb,
				};
				Report {
					message,
					submessage,
					button: Some(button),
					..Default::default()
				}
			}
			ReportType::InfoDbStatusMismatch(status_build, infodb_build) => {
				let message = format!("The MAME Status Update is reporting version {status_build} and the MAME Machine Info output is reporting version {infodb_build}").into();
				let submessage = Some("This is a very unexpected internal error".into());
				let button = Button {
					text: "Retry".into(),
					command: Action::HelpRefreshInfoDb,
				};
				Report {
					message,
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

	pub fn prefs_path(&self) -> &'_ Path {
		&self.fixed.prefs_path
	}

	pub fn last_save_state(&self) -> Option<&'_ str> {
		self.last_save_state.as_deref()
	}

	pub fn set_last_save_state(&mut self, last_save_state: impl Into<Option<Box<str>>>) -> bool {
		self.last_save_state = last_save_state.into();
		true
	}

	pub fn show_console(&self) -> Result<()> {
		let mut console = self.fixed.console.lock().unwrap();
		if console.as_mut().is_none_or(|console| !console.is_running()) {
			*console = Some(Console::new()?);
		}
		Ok(())
	}
}

fn spawn_infodb_build_thread(
	prefs_path: &Path,
	mame_executable_path: &str,
	callback: CommandCallback,
) -> (Job<Result<Option<InfoDb>>>, Canceller) {
	let prefs_path = prefs_path.to_path_buf();
	let mame_executable_path = mame_executable_path.to_string();
	let callback_bubble = ThreadLocalBubble::new(callback);
	let canceller = Canceller::default();
	let job = {
		let canceller = canceller.clone();
		Job::new(move || infodb_build_thread_proc(&prefs_path, &mame_executable_path, callback_bubble, canceller))
	};
	(job, canceller)
}

fn infodb_build_thread_proc(
	prefs_path: &Path,
	mame_executable_path: &str,
	callback_bubble: ThreadLocalBubble<CommandCallback>,
	canceller: Canceller,
) -> Result<Option<InfoDb>> {
	// progress messages need to be throttled
	let mut throttle = Throttle::new(Duration::from_millis(100), 1);

	// lambda to invoke a command on the main event loop; there is some nontrivial stuff here
	// because of the need to put the callback in the "bubble" as well as to ensure that we
	// don't invoke the command if the user cancelled
	let cancelled_clone = canceller.clone();
	let invoke_command = move |command| {
		let callback_bubble = callback_bubble.clone();
		let cancelled_clone = cancelled_clone.clone();
		invoke_from_event_loop(move || {
			if cancelled_clone.status().is_continue() {
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
			let command = Action::InfoDbBuildProgress { machine_description };
			invoke_command_clone(command);
		}

		// have we cancelled?
		canceller.status()
	};

	// invoke MAME with `-listxml`
	let result = InfoDb::from_child_process(mame_executable_path, callback);

	// save the InfoDb (if we got one)
	if let Ok(Some(info_db)) = &result {
		let _ = info_db.save(prefs_path, mame_executable_path);
	}

	// signal that we're done
	invoke_command(Action::InfoDbBuildComplete);

	// and return the result
	result
}

#[allow(clippy::type_complexity)]
fn validate_and_update_status(
	status: Option<&Rc<Status>>,
	pending_status: Option<&Rc<Status>>,
	update: Option<Update>,
	info_db: &InfoDb,
) -> (Option<Rc<Status>>, Option<Rc<Status>>, Result<(), ValidationError>) {
	let current_status = status.or(pending_status).map(|x| x.as_ref());

	let result = if let Some(update) = update.as_ref() {
		update.validate(info_db)
	} else if let Some(current_status) = current_status {
		current_status.validate(info_db)
	} else {
		Ok(())
	};

	// merge the status (if appropriate)
	if let Some(update) = update {
		let merged_status = Status::new(current_status, update);
		let merged_status = Some(Rc::new(merged_status));
		if result.is_ok() {
			(merged_status, None, result)
		} else {
			(status.cloned(), merged_status, result)
		}
	} else {
		(status.cloned(), pending_status.cloned(), result)
	}
}
