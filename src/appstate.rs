use std::borrow::Cow;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;
use anyhow::Result;
use slint::invoke_from_event_loop;
use throttle::Throttle;

use crate::appcommand::AppCommand;
use crate::info::InfoDb;
use crate::job::Job;
use crate::prefs::pathtype::PathType;
use crate::prefs::PreflightProblem;
use crate::prefs::PrefsPaths;
use crate::runtime::command::MameCommand;
use crate::runtime::session::spawn_mame_session_thread;
use crate::runtime::MameStderr;
use crate::runtime::MameWindowing;
use crate::status::Status;
use crate::status::Update;
use crate::status::UpdateXmlProblem;
use crate::status::ValidationError;
use crate::threadlocalbubble::ThreadLocalBubble;
use crate::version::MameVersion;

#[derive(Clone)]
pub struct AppState {
	paths: Rc<PrefsPaths>,
	info_db_build: Option<InfoDbBuild>,
	live: Option<Live>,
	failure: Option<Rc<Failure>>,
	pending_shutdown: bool,
	fixed: Rc<Fixed>,
}

/// Represents the state of an InfoDb build (-listxml) job
#[derive(Clone)]
struct InfoDbBuild {
	job: Job<Result<Option<InfoDb>>>,
	cancelled: Arc<AtomicBool>,
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
	job: Job<Result<()>>,
	command_sender: Option<Arc<Sender<Cow<'static, str>>>>,
	status: Option<Rc<Status>>,
	pending_status: Option<Rc<Status>>,
	pending_paths_update: Option<Rc<PrefsPaths>>,
	pending_restart: bool,
}

#[derive(Debug)]
enum Failure {
	Preflight(Vec<PreflightProblem>),
	SessionError(Error),
	StatusValidationProblem(ValidationError),
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
			paths,
			info_db_build: None,
			live: None,
			failure: None,
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

	pub fn activate(&self) -> Option<Self> {
		// if we already have a session (in any form, we're already active) or if we're shutting down, don't proceed
		if self.live.as_ref().is_some_and(|live| live.session.is_some()) || self.pending_shutdown {
			return None;
		}

		// get or load the InfoDb
		let info_db = self.info_db().cloned().or_else(|| {
			self.paths
				.as_ref()
				.mame_executable
				.as_deref()
				.and_then(|mame_executable_path| InfoDb::load(&self.fixed.prefs_path, mame_executable_path).ok())
				.map(Rc::new)
		});

		if let Some(info_db) = info_db {
			let preflight_problems = self.paths.preflight();
			let session = preflight_problems.is_empty().then(|| {
				let (job, command_sender) = spawn_mame_session_thread(
					self.paths.as_ref(),
					&self.fixed.mame_windowing,
					self.fixed.mame_stderr,
					self.fixed.callback.clone(),
				);
				let command_sender = Some(Arc::new(command_sender));
				Session {
					job,
					command_sender,
					status: None,
					pending_status: None,
					pending_paths_update: None,
					pending_restart: false,
				}
			});

			let failure = session
				.is_none()
				.then(|| Rc::new(Failure::Preflight(preflight_problems)));
			let new_state = Self {
				live: Some(Live { info_db, session }),
				failure,
				..self.clone()
			};
			Some(new_state)
		} else {
			// we don't have InfoDb; force a rebuild
			self.infodb_rebuild()
		}
	}

	pub fn infodb_rebuild(&self) -> Option<Self> {
		if self.info_db_build.is_some() {
			return None;
		}

		// quick run of preflight
		let preflight_problems = self.paths.preflight();
		let new_state = if preflight_problems
			.iter()
			.any(|x| x.problem_type() == Some(PathType::MameExecutable))
		{
			let failure = Failure::Preflight(preflight_problems);
			let failure = Some(Rc::new(failure));
			Self {
				failure,
				..self.clone()
			}
		} else {
			let prefs_path = &self.fixed.prefs_path;
			let mame_executable_path = self.paths.mame_executable.as_deref().unwrap();
			let callback = self.fixed.callback.clone();
			let (job, cancelled) = spawn_infodb_build_thread(prefs_path, mame_executable_path, callback);
			let info_db_build = InfoDbBuild {
				job,
				cancelled,
				machine_description: None,
			};
			Self {
				info_db_build: Some(info_db_build),
				..self.clone()
			}
		};
		Some(new_state)
	}

	// update paths and refresh MAME if needed
	pub fn update_paths(&self, paths: &Rc<PrefsPaths>) -> Option<Self> {
		if self.paths.as_ref() == paths.as_ref() {
			return None;
		}

		// shutdown the live session if we have one; other wise drop it all
		let live = self
			.live
			.as_ref()
			.and_then(|live| live.session.as_ref().map(|session| (live.info_db.clone(), session)))
			.map(|(info_db, old_session)| {
				let new_session = Session {
					command_sender: None,
					pending_paths_update: Some(paths.clone()),
					pending_restart: true,
					..old_session.clone()
				};
				Live {
					info_db,
					session: Some(new_session),
				}
			});

		// create the new state
		let paths = if live.is_some() {
			self.paths.clone()
		} else {
			paths.clone()
		};
		let new_state = Self {
			live,
			paths,
			..self.clone()
		};

		// attempt to reactivate and return
		let new_state = new_state.activate().unwrap_or(new_state);
		Some(new_state)
	}

	/// Issues a command to MAME
	pub fn issue_command(&self, command: MameCommand<'_>) {
		let session = self.live.as_ref().unwrap().session.as_ref().unwrap();
		if let Some(command_sender) = session.command_sender.as_deref() {
			command_sender.send(command.text()).unwrap();
		}
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
					// no session, no problem!
					(None, None)
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
		let failure = failure.map(Rc::new);
		Self {
			live,
			failure,
			info_db_build: None,
			..self.clone()
		}
	}

	/// Apply a `worker_ui` status update
	pub fn status_update(&self, update: Update) -> Option<Self> {
		let live = self.live.as_ref().unwrap();
		let session = live.session.as_ref().unwrap();

		// ignore status updates when we're shutting down
		session.command_sender.as_ref()?;

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
			Err(e) => (Some(Rc::new(Failure::StatusValidationProblem(e))), false),
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
		let failure = failure.or_else(|| self.failure.clone());
		let new_state: AppState = Self {
			live: Some(new_live),
			failure,
			..self.clone()
		};

		// kick off an InfoDb rebuild if appropriate
		let new_state = rebuild_info_db
			.then(|| new_state.infodb_rebuild())
			.flatten()
			.unwrap_or(new_state);

		// and return the new state
		Some(new_state)
	}

	/// The MAME session ended; return a new state
	pub fn session_ended(&self) -> Self {
		// access the "live" and the session
		let live = self.live.as_ref().unwrap();
		let session = live.session.as_ref().unwrap();

		// join the thread and get the result
		let result = session.job.join().unwrap();

		// if we failed, we have to report the error
		let failure = if let Err(e) = result {
			Some(Rc::new(Failure::SessionError(e)))
		} else {
			None
		};

		// there might be a pending paths update
		let pending_paths = session.pending_paths_update.as_ref();

		// do we need to restart ourselves afterwards?
		let pending_restart = session.pending_restart && failure.is_none();

		// create the new state
		let new_live = Live {
			session: None,
			..live.clone()
		};
		let new_state = Self {
			live: Some(new_live),
			failure,
			..self.clone()
		};

		// apply any pending paths update
		let new_state = pending_paths
			.and_then(|paths| new_state.update_paths(paths))
			.unwrap_or(new_state);

		// if there is a pending restart, kick it off - in any case after this we're done
		pending_restart
			.then(|| new_state.activate())
			.flatten()
			.unwrap_or(new_state)
	}

	pub fn shutdown(&self) -> Option<Self> {
		(!self.pending_shutdown).then(|| {
			let live = self.live.as_ref().map(|live| {
				let session = live.session.as_ref().map(|session| Session {
					command_sender: None,
					..session.clone()
				});
				Live {
					session,
					..live.clone()
				}
			});
			Self {
				pending_shutdown: true,
				live,
				..self.clone()
			}
		})
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
							let command = AppCommand::SettingsPaths(Some(path_type));
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
					command: AppCommand::ReactivateMame
				};
				Report {
					message: "MAME has errored".into(),
					submessage: Some(format!("{error}").into()),
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
			ReportType::InfoDbStatusMismatch(status_build, infodb_build) => {
				let message = format!("The MAME Status Update is reporting version {status_build} and the MAME Machine Info output is reporting version {infodb_build}").into();
				let submessage = Some("This is a very unexpected internal error".into());
				let button = Button {
					text: "Retry".into(),
					command: AppCommand::HelpRefreshInfoDb,
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

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::AppState;

	#[test_case(0, AppState::bogus(), false)]
	#[test_case(1, AppState::bogus().shutdown(), true)]
	pub fn is_shutdown(_index: usize, state: impl Into<Option<AppState>>, expected: bool) {
		let state = state.into().unwrap();
		let actual = state.is_shutdown();
		assert_eq!(expected, actual);
	}
}
