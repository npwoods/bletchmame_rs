use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::Error;
use anyhow::Result;
use slint::invoke_from_event_loop;
use smol_str::SmolStr;
use smol_str::ToSmolStr;
use smol_str::format_smolstr;
use throttle::Throttle;

use crate::action::Action;
use crate::canceller::Canceller;
use crate::console::Console;
use crate::info::InfoDb;
use crate::job::Job;
use crate::prefs::PreflightProblem;
use crate::runtime::MameStartArgs;
use crate::runtime::MameStderr;
use crate::runtime::args::MameArguments;
use crate::runtime::args::MameArgumentsResult;
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
	mame_args_result: MameArgumentsResult,
	info_db_build: Option<InfoDbBuild>,
	live: Option<Live>,
	failure: Option<Failure>,
	last_save_state: Option<Box<str>>,
	pending_shutdown: bool,
	pending_start: Option<MameStartArgs>,
	fixed: Fixed,
}

/// Represents the state of an InfoDb build (-listxml) job
#[derive(Clone)]
struct InfoDbBuild {
	job: Job<Result<Option<InfoDb>>>,
	canceller: Canceller,
	machine_description: Option<String>,
}

/// Represents so-called "live" state; we have an InfoDb and maybe a build
#[derive(Clone)]
struct Live {
	info_db: Rc<InfoDb>,
	session: Option<Session>,
}

/// Represents a session and associated communication
#[derive(Clone)]
struct Session {
	job: Job<SessionResult<()>>,
	command_sender: Option<Arc<Sender<MameCommand>>>,
	status: Option<Rc<Status>>,
	pending_status: Option<Rc<Status>>,
	pending_mame_args_result_update: Option<MameArgumentsResult>,
	pending_restart: bool,
}

#[derive(Debug)]
enum Failure {
	Preflight(Rc<[PreflightProblem]>),
	SessionError(SessionError),
	StatusValidationProblem(ValidationError),
	InfoDbBuild(Error),
	InfoDbBuildCancelled,
}

struct Fixed {
	prefs_path: PathBuf,
	mame_stderr: MameStderr,
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
		mame_args_result: MameArgumentsResult,
		mame_stderr: MameStderr,
		callback: impl Fn(Action) + 'static,
	) -> Self {
		let console = Arc::new(Mutex::new(None));
		let callback = Rc::from(callback);
		let fixed = Fixed {
			prefs_path,
			mame_stderr,
			console,
			callback,
		};
		Self {
			mame_args_result,
			info_db_build: None,
			live: None,
			failure: None,
			last_save_state: None,
			pending_shutdown: false,
			pending_start: None,
			fixed,
		}
	}

	/// Creates a "bogus" AppState that should never be used
	pub fn bogus() -> Self {
		Self::new(
			"".into(),
			Ok(MameArguments::default()),
			MameStderr::Capture,
			|_| unreachable!(),
		)
	}

	pub fn activate(&mut self) -> bool {
		// if we already have a session (in any form, we're already active) or if we're shutting down, don't proceed
		if self.live.as_ref().is_some_and(|live| live.session.is_some()) || self.pending_shutdown {
			return false;
		}

		// get or load the InfoDb
		let info_db = self.info_db().cloned().or_else(|| {
			let mame_executable_path = self.mame_args_result.as_ref().as_ref().ok()?.program.as_str();
			let info_db = InfoDb::load(&self.fixed.prefs_path, mame_executable_path).ok()?;
			Some(Rc::new(info_db))
		});

		if let Some(info_db) = info_db {
			let (session, failure) = match self.mame_args_result.clone() {
				Ok(mame_args) => {
					let session = self.start_session(mame_args);
					(Some(session), None)
				}
				Err(e) => {
					let failure = Failure::Preflight(e.preflight_problems);
					(None, Some(failure))
				}
			};

			self.live = Some(Live { info_db, session });
			self.failure = failure;
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
			pending_status: None,
			pending_mame_args_result_update: None,
			pending_restart: false,
		}
	}

	pub fn infodb_rebuild(&mut self) -> bool {
		if self.info_db_build.is_some() {
			return false;
		}

		// access the MAME executable path (or preflight errors if we don't have them)
		let mame_executable_path_result = match self.mame_args_result.as_ref() {
			Ok(mame_args) => Ok(mame_args.program.as_str()),
			Err(e) => {
				if let Some(program) = &e.program {
					Ok(program.as_str())
				} else {
					Err(&e.preflight_problems)
				}
			}
		};

		// and based on whether we had success or failure, built the new state
		match mame_executable_path_result {
			Ok(mame_executable_path) => {
				let prefs_path = &self.fixed.prefs_path;
				let callback = self.fixed.callback.clone();
				let (job, canceller) = spawn_infodb_build_thread(prefs_path, mame_executable_path, callback);
				let info_db_build = InfoDbBuild {
					job,
					canceller,
					machine_description: None,
				};
				self.info_db_build = Some(info_db_build);
			}

			Err(preflight_problems) => {
				assert!(!preflight_problems.is_empty());
				let preflight_problems = preflight_problems.clone();
				let failure = Failure::Preflight(preflight_problems);
				self.failure = Some(failure);
			}
		};
		true
	}

	// update MAME arguments and refresh MAME if needed
	pub fn update_mame_args_result(&mut self, mame_args_result: MameArgumentsResult) -> bool {
		if self.mame_args_result == mame_args_result {
			// no changes? nothing to do
			return false;
		}

		// shutdown the live session if we have one; other wise drop it all
		let live = self
			.live
			.as_ref()
			.and_then(|live| live.session.as_ref().map(|session| (live.info_db.clone(), session)))
			.map(|(info_db, old_session)| {
				let new_session = Session {
					command_sender: None,
					pending_mame_args_result_update: Some(mame_args_result.clone()),
					pending_restart: true,
					..old_session.clone()
				};
				Live {
					info_db,
					session: Some(new_session),
				}
			});

		// create the new state
		let mame_args_result = if live.is_some() {
			self.mame_args_result.clone()
		} else {
			mame_args_result.clone()
		};

		self.live = live;
		self.mame_args_result = mame_args_result;

		// attempt to reactivate and return
		self.activate();
		true
	}

	pub fn reset(&mut self) -> bool {
		let live = self
			.live
			.as_ref()
			.and_then(|live| live.session.as_ref().map(|session| (live.info_db.clone(), session)))
			.map(|(info_db, old_session)| {
				let new_session = Session {
					command_sender: None,
					pending_restart: true,
					..old_session.clone()
				};
				Live {
					info_db,
					session: Some(new_session),
				}
			});
		self.live = live;

		// attempt to reactivate and return
		self.activate();
		true
	}

	pub fn set_pending_start(&mut self, pending_start: MameStartArgs) -> bool {
		self.pending_start = Some(pending_start);
		true
	}

	pub fn issue_pending_start_if_possible(&mut self) -> bool {
		let Some(live) = self.live.as_ref() else {
			return false;
		};
		let Some(session) = live.session.as_ref() else {
			return false;
		};
		if session.status.is_none() {
			return false;
		}
		let Some(command_sender) = session.command_sender.as_deref() else {
			return false;
		};
		let Some(pending_start) = self.pending_start.as_ref() else {
			return false;
		};
		let command = MameCommand::start(pending_start);
		command_sender.send(command).unwrap();

		self.pending_start = None;
		true
	}

	/// Issues a command to MAME
	pub fn issue_command(&self, command: MameCommand) {
		let session = self.live.as_ref().unwrap().session.as_ref().unwrap();
		if let Some(command_sender) = session.command_sender.as_deref() {
			command_sender.send(command).unwrap();
		}
	}

	pub fn infodb_build_progress(&mut self, machine_description: String) -> bool {
		let info_db_build = InfoDbBuild {
			machine_description: Some(machine_description),
			..self.info_db_build.as_ref().unwrap().clone()
		};
		self.info_db_build = Some(info_db_build);
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
		let (live, failure) = match (result, self.live.as_ref()) {
			// the rebuild succeeded, incorporate it into the new `Live`
			(Ok(Some(info_db)), live) => {
				let old_session = live.and_then(|live| live.session.as_ref());
				let (new_session, failure) = if let Some(old_session) = old_session {
					// we do have a session; we need to validate and apply any pending status update
					let (status, pending_status, result) = validate_and_update_status(
						old_session.status.as_ref(),
						old_session.pending_status.as_ref(),
						None,
						&info_db,
					);
					let (command_sender, failure) = if let Err(e) = result {
						(None, Some(Failure::StatusValidationProblem(e)))
					} else {
						(old_session.command_sender.clone(), None)
					};

					let new_session = Session {
						status,
						pending_status,
						command_sender,
						..old_session.clone()
					};
					(Some(new_session), failure)
				} else {
					// no session; create a new one
					let mame_args = self.mame_args_result.as_ref().unwrap().clone();
					let session = self.start_session(mame_args);
					(Some(session), None)
				};
				let new_live = Live {
					info_db: Rc::new(info_db),
					session: new_session,
				};
				(Some(new_live), failure)
			}

			// the user cancelled and we're not live - show the cancel as a "failure"
			(Ok(None), None) => (None, Some(Failure::InfoDbBuildCancelled)),

			// the user cancelled but we're live - no need to report anything
			(Ok(None), Some(live)) => (Some(live.clone()), None),

			// an unexpected error occurred; shut down the live session (if any) and report the error
			(Err(e), live) => {
				let live = live.map(|live| {
					let session = live.session.as_ref().map(|session| Session {
						command_sender: None,
						..session.clone()
					});
					Live {
						session,
						..live.clone()
					}
				});
				let failure = Some(Failure::InfoDbBuild(e));
				(live, failure)
			}
		};

		// and return
		self.live = live;
		self.failure = failure;
		self.info_db_build = None;
		true
	}

	/// Apply a `worker_ui` status update
	pub fn status_update(&mut self, update: Update) -> bool {
		let live = self.live.as_ref().unwrap();
		let session = live.session.as_ref().unwrap();

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
		let new_session = Session {
			status: new_status,
			pending_status: new_pending_status,
			..session.clone()
		};
		let new_live = Live {
			session: Some(new_session),
			..live.clone()
		};
		self.live = Some(new_live);
		if let Some(failure) = failure {
			self.failure = Some(failure);
		}

		// kick off an InfoDb rebuild if appropriate
		if rebuild_info_db {
			self.infodb_rebuild();
		}
		true
	}

	/// The MAME session ended; return a new state
	pub fn session_ended(&mut self) -> bool {
		// access the "live" and the session
		let live = self.live.as_mut().unwrap();
		let session = live.session.as_mut().unwrap();

		// join the thread and get the result
		let result = session.job.join().unwrap();

		// if we failed, we have to report the error
		let failure = result.err().map(Failure::SessionError);

		// there might be a pending paths update
		let pending_mame_args_result = session.pending_mame_args_result_update.take();

		// do we need to restart ourselves afterwards?
		let pending_restart = session.pending_restart && failure.is_none();

		// create the new state
		if let Some(live) = self.live.as_mut() {
			live.session = None;
		}
		self.failure = failure;

		// apply any pending paths update
		if let Some(pending_mame_args_result) = pending_mame_args_result {
			self.update_mame_args_result(pending_mame_args_result);
		}

		// if there is a pending restart, kick it off - in any case after this we're done
		if pending_restart {
			self.activate();
		}
		true
	}

	pub fn shutdown(&mut self) -> bool {
		if self.pending_shutdown {
			return false;
		}

		self.pending_shutdown = true;
		if let Some(session) = self.live.as_mut().and_then(|live| live.session.as_mut()) {
			session.command_sender = None;
		}
		true
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
			ShuttingDown,
			PreflightFailure(&'a [PreflightProblem]),
			SessionError(&'a SessionError),
			InvalidStatusUpdate(&'a [UpdateXmlProblem]),
			InfoDbBuildFailure(Option<&'a Error>),
			InfoDbStatusMismatch(&'a MameVersion, &'a MameVersion),
		}

		// upfront logic to determine the type of report presented, if any; keep
		// this logic distinct from the mechanics of displaying the report
		let is_starting_up = self
			.live
			.as_ref()
			.and_then(|live| live.session.as_ref())
			.map(|session| session.status.is_none())
			.unwrap_or_default();
		let report_type = match (
			self.info_db_build.as_ref(),
			self.failure.as_ref(),
			is_starting_up,
			self.pending_shutdown,
		) {
			(Some(info_db_build), _, _, _) => {
				Some(ReportType::InfoDbBuild(info_db_build.machine_description.as_deref()))
			}
			(None, _, _, true) => Some(ReportType::ShuttingDown),
			(None, _, true, false) => Some(ReportType::Resetting),
			(None, Some(Failure::Preflight(preflight_problems)), false, false) => {
				Some(ReportType::PreflightFailure(preflight_problems.as_ref()))
			}
			(None, Some(Failure::SessionError(e)), false, false) => Some(ReportType::SessionError(e)),
			(None, Some(Failure::StatusValidationProblem(ValidationError::Invalid(e))), false, false) => {
				Some(ReportType::InvalidStatusUpdate(e.as_slice()))
			}
			(
				None,
				Some(Failure::StatusValidationProblem(ValidationError::VersionMismatch(status_build, infodb_build))),
				false,
				false,
			) => Some(ReportType::InfoDbStatusMismatch(status_build, infodb_build)),
			(None, Some(Failure::InfoDbBuild(e)), false, false) => Some(ReportType::InfoDbBuildFailure(Some(e))),
			(None, Some(Failure::InfoDbBuildCancelled), false, false) => Some(ReportType::InfoDbBuildFailure(None)),
			(None, None, false, false) => None,
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

	pub fn is_shutdown(&self) -> bool {
		self.pending_shutdown
			&& self.info_db_build.is_none()
			&& self.live.as_ref().is_none_or(|live| live.session.is_none())
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
