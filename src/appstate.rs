use std::mem::replace;
use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

use anyhow::Error;
use anyhow::Result;
use more_asserts::assert_gt;
use slint::SharedString;
use slint::ToSharedString;
use slint::invoke_from_event_loop;
use smol_str::SmolStr;
use throttle::Throttle;
use tracing::debug;
use tracing::info;

use crate::action::Action;
use crate::audit::Asset;
use crate::audit::AuditResult;
use crate::audit::AuditSeverity;
use crate::info::InfoDb;
use crate::interaction_monitor::InteractionMonitor;
use crate::job::Canceller;
use crate::job::Job;
use crate::mconfig::MachineConfig;
use crate::models::audit::audit_static_model;
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
use crate::ui::AuditFailureReportInfo;
use crate::ui::Icons;
use crate::ui::InfoDbStatusMismatchReportInfo;
use crate::ui::InvalidStatusUpdateReportInfo;
use crate::ui::PreflightFailureReportInfo;
use crate::ui::PreflightFailureReportProblem;
use crate::ui::SessionErrorReportInfo;
use crate::util::IteratorExt as _;
use crate::version::MameVersion;

use crate::runtime::session::Error as SessionError;
use crate::runtime::session::Result as SessionResult;

pub struct AppState {
	pub preferences: Preferences,
	info_db_build: Option<InfoDbBuild>,
	live: Option<Live>,
	failure: Option<Failure>,
	last_save_state: Option<Box<str>>,
	fixed: Fixed,
}

/// Represents the state of an InfoDb build (-listxml) job
struct InfoDbBuild {
	job: Job<Result<Option<InfoDb>>>,
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
	video: Option<PrefsVideo>,
	status: Option<Rc<Status>>,
	pending_status: Option<Rc<Status>>,
	session_state: SessionState,
}

#[derive(Debug)]
enum SessionState {
	ShuttingDown,
	Stopping,
	Restarting {
		start_args: Option<Arc<MameStartArgs>>,
	},
	Active {
		command_sender: Sender<MameCommand>,
		active_state: SessionActiveState,
	},
}

#[derive(Debug)]
enum SessionActiveState {
	Normal,
	EmuStarting,
	EmuStopping,
	Auditing {
		job: Job<AuditJobResult>,
		start_args: Arc<MameStartArgs>,
		current_asset_name: Option<SmolStr>,
		current_progress: f32,
	},
}

#[derive(Debug)]
enum Failure {
	Preflight(Box<[PreflightProblem]>),
	SessionError(SessionError),
	InfoDbStatusMismatch {
		status_build: MameVersion,
		infodb_build: MameVersion,
	},
	InvalidStatusUpdate(Vec<UpdateXmlProblem>),
	InfoDbBuild(Error),
	InfoDbBuildCancelled,
	AuditResults {
		items: Box<[(Asset, AuditResult)]>,
		proceed_action: Action,
	},
	AuditError(Error),
	AuditCancelled,
}

struct Fixed {
	prefs_path: PathBuf,
	mame_stderr: MameStderr,
	mame_windowing: MameWindowing,
	interaction_monitor: Arc<Mutex<Option<InteractionMonitor>>>,
	callback: ActionCallback,
}

#[derive(Debug)]
pub enum AuditJobResult {
	Success,
	Cancelled,
	Failed(Box<[(Asset, AuditResult)]>),
}

type ActionCallback = Rc<dyn Fn(Action) + 'static>;

// progress messages should be throttled
const PROGRESS_THROTTLE_TIMEOUT: Duration = Duration::from_millis(100);

// debugging feature to make auditing easier to debug
const AUDIT_DELAY: Option<Duration> = None;

#[derive(Debug)]
pub enum Report {
	// session related reports
	SessionStarting,
	SessionRestarting,
	SessionRestartingForEmu,
	SessionShuttingDown,

	// messages for an emulation starting up or shutting down
	EmuStarting,
	EmuStopping,
	Auditing {
		asset_name: Option<SharedString>,
		progress: f32,
	},

	// InfoDb building
	InfoDbBuild {
		machine_description: Option<SharedString>,
	},

	// failure conditions
	PreflightFailure(PreflightFailureReportInfo),
	SessionError(SessionErrorReportInfo),
	InfoDbStatusMismatch(InfoDbStatusMismatchReportInfo),
	InvalidStatusUpdate(InvalidStatusUpdateReportInfo),
	InfoDbBuildFailure(SharedString),
	InfoDbBuildCancelled,
	AuditFailure(AuditFailureReportInfo),
	AuditError(SharedString),
	AuditCancelled,
}

impl AppState {
	/// Creates an initial `AppState`
	pub fn new(
		prefs_path: PathBuf,
		mame_stderr: MameStderr,
		mame_windowing: MameWindowing,
		callback: impl Fn(Action) + 'static,
	) -> Self {
		let interaction_monitor = Arc::new(Mutex::new(None));
		let callback = Rc::from(callback);
		let fixed = Fixed {
			prefs_path,
			mame_stderr,
			mame_windowing,
			interaction_monitor,
			callback,
		};
		Self {
			preferences: Preferences::default(),
			info_db_build: None,
			live: None,
			failure: None,
			last_save_state: None,
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

	fn make_mame_args(&mut self, video: Option<&PrefsVideo>) -> std::result::Result<MameArguments, ()> {
		// clear out any failures
		self.failure = None;

		// create MAME arguments
		let mame_args_result = MameArguments::new(&self.preferences, video, &self.fixed.mame_windowing, false);

		// if we failed, report it
		mame_args_result.map_err(|e| {
			// failed preflight? report the problems
			assert_gt!(e.preflight_problems.len(), 0);
			self.failure = Some(Failure::Preflight(e.preflight_problems));
		})
	}

	pub fn activate(&mut self) -> bool {
		info!("AppState::activate(): starting");

		// clear out any failure
		let had_failure = self.failure.is_some();
		self.failure = None;

		// if we already have a session (in any form, we're already active) or if we're shutting down, don't proceed
		if self.live.as_ref().is_some_and(|live| live.session.is_some()) {
			return had_failure;
		}

		// get or load the InfoDb
		let mame_args_result = self.make_mame_args(None);
		let info_db = self.info_db().cloned().or_else(|| {
			let mame_executable_path = mame_args_result.as_ref().as_ref().ok()?.program.as_str();
			let info_db = InfoDb::load(&self.fixed.prefs_path, mame_executable_path).ok()?;
			Some(Rc::new(info_db))
		});

		if let Some(info_db) = info_db {
			let session = mame_args_result
				.map(|mame_args| self.start_session(mame_args, None))
				.ok();
			self.live = Some(Live { info_db, session });
			true
		} else {
			// we don't have InfoDb; force a rebuild
			self.infodb_rebuild()
		}
	}

	fn start_session(&self, mame_args: MameArguments, start_args: Option<Arc<MameStartArgs>>) -> Session {
		// start the session thread
		let watchdog_timeout = Duration::from_secs(30);
		let (job, command_sender) = spawn_mame_session_thread(
			mame_args,
			self.fixed.mame_stderr,
			watchdog_timeout,
			self.fixed.interaction_monitor.clone(),
			self.fixed.callback.clone(),
		);

		// if we're starting, set the active state accordingly
		let active_state = if start_args.is_some() {
			SessionActiveState::EmuStarting
		} else {
			SessionActiveState::Normal
		};

		// are we starting with a command?
		if let Some(start_args) = start_args.as_deref() {
			let command = MameCommand::start(start_args);
			command_sender.send(command).unwrap();
		}

		// finally return all the state
		let video = start_args.and_then(|x| Arc::unwrap_or_clone(x).video);
		let session_state = SessionState::Active {
			command_sender,
			active_state,
		};
		Session {
			job,
			video,
			status: None,
			pending_status: None,
			session_state,
		}
	}

	pub fn infodb_rebuild(&mut self) -> bool {
		if self.info_db_build.is_some() {
			return false;
		}

		// access the MAME executable path (or preflight errors if we don't have them)
		if let Ok(mame_args) = self.make_mame_args(None) {
			let mame_executable_path = mame_args.program.as_str();
			let prefs_path = &self.fixed.prefs_path;
			let callback = self.fixed.callback.clone();
			let job = spawn_infodb_build_thread(prefs_path, mame_executable_path, callback);
			let info_db_build = InfoDbBuild {
				job,
				machine_description: None,
			};
			self.info_db_build = Some(info_db_build);
		};
		true
	}

	pub fn reset(&mut self) -> bool {
		// if a session is live, set it to stop and restart
		if let Some(session) = self.live.as_mut().and_then(|live| live.session.as_mut()) {
			session.job.cancel();
			session.session_state = SessionState::Restarting { start_args: None };
		}

		// attempt to reactivate and return
		self.activate();
		true
	}

	pub fn start(&mut self, start_args: impl Into<Arc<MameStartArgs>>, skip_audit: bool) -> bool {
		let start_args = start_args.into();
		info!(?start_args, "AppState::start()");

		// access the InfoDb now
		let info_db = self.info_db().unwrap().clone();

		// access the live session (which had better be present)
		let session = self.live.as_mut().unwrap().session.as_mut().unwrap();

		// we expect to have an active session
		let SessionState::Active {
			active_state,
			command_sender,
			..
		} = &mut session.session_state
		else {
			panic!("AppState::start() called without active session");
		};

		if !skip_audit {
			// start an auditing session
			let rom_paths = self.preferences.paths.roms.clone();
			let sample_paths = self.preferences.paths.samples.clone();
			let software_list_paths = &self.preferences.paths.software_lists;
			let callback = self.fixed.callback.clone();
			let job = match spawn_audit(
				info_db,
				rom_paths,
				sample_paths,
				software_list_paths,
				AUDIT_DELAY,
				&start_args,
				callback,
			) {
				Ok(job) => job,
				Err(e) => {
					self.failure = Some(Failure::AuditError(e));
					return true;
				}
			};

			// and set up the state
			*active_state = SessionActiveState::Auditing {
				job,
				start_args,
				current_asset_name: None,
				current_progress: 0.0,
			};
		} else if start_args.video == session.video {
			// it does, lets go!
			let command = MameCommand::start(&start_args);

			// dispatch the command
			command_sender.send(command).unwrap();

			// and set the state to "starting"
			*active_state = SessionActiveState::EmuStarting;
		} else {
			// it doesn't; we need to restart
			session.session_state = SessionState::Restarting {
				start_args: Some(start_args),
			};
		}
		self.failure = None;
		true
	}

	pub fn audit_progress(&mut self, asset_name: SmolStr, progress: f32) -> bool {
		// access the live session (which had better be present)
		let session = self.live.as_mut().unwrap().session.as_mut().unwrap();

		// access the auditing session
		let SessionState::Active {
			active_state: SessionActiveState::Auditing {
				current_asset_name,
				current_progress,
				..
			},
			..
		} = &mut session.session_state
		else {
			// should never happen because `Action::AuditProgress` will only be invoked by an auditing job
			panic!("audit_progress() called without session");
		};

		// record progress
		*current_asset_name = Some(asset_name);
		*current_progress = progress;
		true
	}

	pub fn audit_cancel(&mut self) -> bool {
		// access the live session (which had better be present)
		let session = self.live.as_mut().unwrap().session.as_mut().unwrap();

		// access the auditing session
		let SessionState::Active {
			active_state: SessionActiveState::Auditing { job, .. },
			..
		} = &session.session_state
		else {
			// should never happen because `Action::AuditProgress` will only be invoked by an auditing job
			panic!("audit_progress() called without session");
		};

		// cancel the job
		job.cancel();
		true
	}

	pub fn audit_complete(&mut self) -> bool {
		info!("AppState::audit_complete()");

		// access the live session (which had better be present)
		let session = self.live.as_mut().unwrap().session.as_mut().unwrap();

		// and the session should be active
		let SessionState::Active {
			command_sender,
			active_state,
		} = &mut session.session_state
		else {
			// should never happen because `Action::AuditComplete` will only be invoked by an auditing job
			panic!("audit_complete() called without session");
		};

		// ...and the active session should be auditing
		let active_state_moved = replace(active_state, SessionActiveState::Normal);
		let SessionActiveState::Auditing { job, start_args, .. } = active_state_moved else {
			// should never happen because `Action::AuditComplete` will only be invoked by an auditing job
			panic!("audit_complete() called without auditing session");
		};

		// get the results
		let audit_result = job.join();

		// how did the audit go?
		match audit_result {
			AuditJobResult::Success => {
				// the audit succeeded; not check to seee if the video matchesdoes the video match?
				if start_args.video == session.video {
					// it does, lets go!
					let command = MameCommand::start(&start_args);

					// dispatch the command
					command_sender.send(command).unwrap();

					// and set the state to "starting"
					*active_state = SessionActiveState::EmuStarting;
				} else {
					// it doesn't; we need to restart
					session.session_state = SessionState::Restarting {
						start_args: Some(start_args),
					};
				}
			}
			AuditJobResult::Cancelled => {
				self.failure = Some(Failure::AuditCancelled);
			}
			AuditJobResult::Failed(items) => {
				let proceed_action = Action::StartSkipAudit(start_args);
				let failure = Failure::AuditResults { items, proceed_action };
				self.failure = Some(failure);
			}
		}

		true
	}

	pub fn stop(&mut self) -> bool {
		// access the live session (which had better be present)
		let session = self.live.as_mut().unwrap().session.as_mut().unwrap();

		// get the active session (if we don't have one, we're already stopping)
		let SessionState::Active {
			command_sender,
			active_state,
		} = &mut session.session_state
		else {
			return false;
		};

		// are we already stopping?
		if matches!(*active_state, SessionActiveState::EmuStopping) {
			return false;
		}

		// send the stop command
		command_sender.send(MameCommand::stop()).unwrap();

		// we're now stopping
		*active_state = SessionActiveState::EmuStopping;

		// and we're done!
		true
	}

	/// Issues a command to MAME
	pub fn issue_command(&self, command: MameCommand) {
		let session = self.live.as_ref().unwrap().session.as_ref().unwrap();
		if let SessionState::Active { command_sender, .. } = &session.session_state {
			command_sender.send(command).unwrap();
		}
	}

	pub fn infodb_build_progress(&mut self, machine_description: String) -> bool {
		self.info_db_build
			.as_mut()
			.expect("infodb_build_progress() invoked with no info_db_build")
			.machine_description = Some(machine_description);
		true
	}

	pub fn infodb_build_cancel(&mut self) -> bool {
		info!("AppState::infodb_build_cancel()");
		self.info_db_build
			.as_ref()
			.expect("infodb_build_cancel() invoked with no info_db_build")
			.job
			.cancel();
		false
	}

	pub fn infodb_build_complete(&mut self) -> bool {
		info!("AppState::infodb_build_complete()");

		// we expect to be in the process of building, and to be able to "take" the job
		let info_db_build = self
			.info_db_build
			.take()
			.expect("infodb_build_complete() invoked with no info_db_build");

		// join the job (which we expect to complete) and digest the result
		let result = info_db_build.job.join();

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

					self.failure = if let Err(error) = result {
						session.session_state = SessionState::Stopping;
						Some(error.into())
					} else {
						None
					};

					session.status = status;
					session.pending_status = pending_status;
				} else {
					// no session; create a new one
					if let Ok(mame_args) = self.make_mame_args(None) {
						let session = self.start_session(mame_args, None);
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
					session.session_state = SessionState::Stopping;
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

		// ignore status updates if the session is not active
		let SessionState::Active { active_state, .. } = &mut session.session_state else {
			return false;
		};

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
			Err(e) => (Some(e.into()), false),
		};

		// and munge this into the new state
		session.status = new_status;
		session.pending_status = new_pending_status;
		if let Some(failure) = failure {
			self.failure = Some(failure);
		}

		// if we have an active session, we may need to alter the active state
		let is_running = session.status.as_deref().is_some_and(|s| s.running.is_some());
		let now_normal = match active_state {
			SessionActiveState::EmuStarting => is_running,
			SessionActiveState::EmuStopping => !is_running,
			_ => false,
		};
		if now_normal {
			*active_state = SessionActiveState::Normal
		}
		if !is_running {
			self.last_save_state = None;
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
		let session = live.session.take().unwrap();

		// join the thread and get the result
		let result = session.job.join();

		// if we failed, we have to report the error
		let failure = result.err().map(Failure::SessionError);

		// identify activities that need to happen after the session ends
		let (reactivate, start_args, result) = match session.session_state {
			SessionState::Restarting { start_args } => (true, start_args, ControlFlow::Continue(())),
			SessionState::ShuttingDown => (false, None, ControlFlow::Break(())),
			_ => (false, None, ControlFlow::Continue(())),
		};

		// identify failures
		self.failure = failure;

		// do we need to restart the session?
		if reactivate {
			let video = start_args.as_ref().and_then(|x| x.video.as_ref());
			let Ok(mame_args) = self.make_mame_args(video) else {
				return ControlFlow::Continue(());
			};
			let session = self.start_session(mame_args, start_args);
			self.live.as_mut().unwrap().session = Some(session);
		}

		// and we're done!
		result
	}

	pub fn shutdown(&mut self) -> ControlFlow<()> {
		let session = self.live.as_mut().and_then(|live| live.session.as_mut());
		if let Some(session) = session {
			session.session_state = SessionState::ShuttingDown;
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
					.map(|running| {
						live.info_db
							.machines()
							.find(&running.machine_name)
							.unwrap()
							.description()
					})
			})
			.unwrap_or_default()
	}

	pub fn report(&self, icons: Icons<'_>) -> Option<Report> {
		// lots of gnarly logic here
		if let Some(info_db_build) = self.info_db_build.as_ref() {
			// report that we have an active InfoDb build
			let report = Report::InfoDbBuild {
				machine_description: info_db_build.machine_description.as_deref().map(SharedString::from),
			};
			Some(report)
		} else if let Some(failure) = self.failure.as_ref() {
			// report that something out there failed
			match failure {
				Failure::Preflight(problems) => {
					let problems = problems
						.iter()
						.map(PreflightFailureReportProblem::from)
						.collect_model_rc();
					let info = PreflightFailureReportInfo { problems };
					Some(Report::PreflightFailure(info))
				}
				Failure::SessionError(error) => {
					let error_message = error.to_shared_string();
					let mame_stderr_text = error.mame_stderr_text.as_deref().unwrap_or_default().into();
					let exit_code = error.exit_code.map(|c| c.to_shared_string()).unwrap_or_default();
					let info = SessionErrorReportInfo {
						error_message,
						mame_stderr_text,
						exit_code,
					};
					Some(Report::SessionError(info))
				}
				Failure::InfoDbStatusMismatch {
					status_build,
					infodb_build,
				} => {
					let status_build = status_build.to_shared_string();
					let infodb_build = infodb_build.to_shared_string();
					let info = InfoDbStatusMismatchReportInfo {
						status_build,
						infodb_build,
					};
					Some(Report::InfoDbStatusMismatch(info))
				}
				Failure::InvalidStatusUpdate(update_xml_problems) => {
					let problems = update_xml_problems
						.iter()
						.map(|problem| problem.to_shared_string())
						.collect_model_rc();
					let info = InvalidStatusUpdateReportInfo { problems };
					Some(Report::InvalidStatusUpdate(info))
				}
				Failure::InfoDbBuild(error) => {
					let error_message = error.to_shared_string();
					Some(Report::InfoDbBuildFailure(error_message))
				}
				Failure::InfoDbBuildCancelled => Some(Report::InfoDbBuildCancelled),
				Failure::AuditResults { items, proceed_action } => {
					let audit_results = items.as_ref();
					let audit_results = audit_static_model(audit_results, icons);
					let proceed_action = proceed_action.encode_for_slint();

					let info = AuditFailureReportInfo {
						audit_results,
						proceed_action,
					};
					Some(Report::AuditFailure(info))
				}
				Failure::AuditError(error) => {
					let error_message = error.to_shared_string();
					Some(Report::AuditError(error_message))
				}
				Failure::AuditCancelled => Some(Report::AuditCancelled),
			}
		} else if let Some(session) = self.live.as_ref().and_then(|live| live.session.as_ref()) {
			match &session.session_state {
				SessionState::ShuttingDown => Some(Report::SessionShuttingDown),
				SessionState::Stopping => None,
				SessionState::Restarting { start_args, .. } => {
					if start_args.is_some() {
						Some(Report::SessionRestartingForEmu)
					} else {
						Some(Report::SessionRestarting)
					}
				}
				SessionState::Active { active_state, .. } => match active_state {
					SessionActiveState::Normal => session.status.is_none().then_some(Report::SessionStarting),
					SessionActiveState::EmuStarting => Some(Report::EmuStarting),
					SessionActiveState::EmuStopping => Some(Report::EmuStopping),
					SessionActiveState::Auditing {
						current_asset_name,
						current_progress,
						..
					} => {
						let report = Report::Auditing {
							asset_name: current_asset_name.as_deref().map(SharedString::from),
							progress: *current_progress,
						};
						Some(report)
					}
				},
			}
		} else {
			None
		}
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

	pub fn show_interaction_monitor(&self) -> Result<()> {
		let mut interaction_monitor = self.fixed.interaction_monitor.lock().unwrap();
		if interaction_monitor
			.as_mut()
			.is_none_or(|interaction_monitor| !interaction_monitor.is_running())
		{
			*interaction_monitor = Some(InteractionMonitor::new()?);
		}
		Ok(())
	}
}

impl From<ValidationError> for Failure {
	fn from(value: ValidationError) -> Self {
		match value {
			ValidationError::VersionMismatch(status_build, infodb_build) => Failure::InfoDbStatusMismatch {
				status_build,
				infodb_build,
			},
			ValidationError::Invalid(update_xml_problems) => Failure::InvalidStatusUpdate(update_xml_problems),
		}
	}
}

fn spawn_infodb_build_thread(
	prefs_path: &Path,
	mame_executable_path: &str,
	callback: ActionCallback,
) -> Job<Result<Option<InfoDb>>> {
	let prefs_path = prefs_path.to_path_buf();
	let mame_executable_path = mame_executable_path.to_string();
	let callback_bubble = ThreadLocalBubble::new(callback);
	Job::new(move |canceller| infodb_build_thread_proc(&prefs_path, &mame_executable_path, callback_bubble, canceller))
}

fn spawn_audit(
	info_db: Rc<InfoDb>,
	rom_paths: Vec<impl AsRef<Path> + Send + 'static>,
	sample_paths: Vec<impl AsRef<Path> + Send + 'static>,
	software_list_paths: &[SmolStr],
	audit_delay: Option<Duration>,
	start_args: &MameStartArgs,
	callback: ActionCallback,
) -> Result<Job<AuditJobResult>> {
	// create the MachineConfig
	let machine_config = MachineConfig::from_mame_start_args(info_db.clone(), start_args)?;

	// create the assets
	let assets = Asset::from_machine_config_and_images(&machine_config, software_list_paths, &start_args.images);

	// progress messages need to be throttled
	let mut throttle = Throttle::new(PROGRESS_THROTTLE_TIMEOUT, 1);

	// create the job
	let callback_bubble = ThreadLocalBubble::new(callback);
	let job = Job::new(move |canceller| {
		let start_instant = Instant::now();

		// we need to invoke actions on the main thread
		let invoke_action = make_invoke_action(callback_bubble, canceller.clone());

		// audit each asset
		let assets_len = assets.len();
		let audit_results = assets
			.into_iter()
			.enumerate()
			.map(|(index, asset)| {
				if canceller.status().is_break() {
					Err(())
				} else {
					// do we need to display a progress message?
					if throttle.accept().is_ok() {
						let progress = (index as f32) / (assets_len as f32);
						let action = Action::AuditProgress(asset.name.as_str().into(), progress);
						invoke_action(action, true);
					}

					// this is only for debugging purposes
					if let Some(audit_delay) = audit_delay {
						sleep(audit_delay);
					}

					// audit the asset
					let audit_result = asset.run_audit(&rom_paths, &sample_paths);
					Ok((asset, audit_result))
				}
			})
			.collect::<std::result::Result<Vec<_>, ()>>();

		// determine what the job result should be
		let job_result = match audit_results {
			Ok(results) => {
				let max_severity = results.iter().map(|(_, audit_result)| audit_result.severity()).max();
				if max_severity.is_none_or(|x| x < AuditSeverity::Fail) {
					AuditJobResult::Success
				} else {
					AuditJobResult::Failed(results.into())
				}
			}
			Err(()) => AuditJobResult::Cancelled,
		};

		// signal completion and return
		invoke_action(Action::AuditComplete, false);
		debug!(duration=?start_instant.elapsed(), "spawn_audit() job");
		job_result
	});
	Ok(job)
}

fn infodb_build_thread_proc(
	prefs_path: &Path,
	mame_executable_path: &str,
	callback_bubble: ThreadLocalBubble<ActionCallback>,
	canceller: Canceller,
) -> Result<Option<InfoDb>> {
	// progress messages need to be throttled
	let mut throttle = Throttle::new(PROGRESS_THROTTLE_TIMEOUT, 1);

	// create a lambda to invoke an action on the main event loop
	let invoke_action = make_invoke_action(callback_bubble, canceller.clone());

	// prep a callback for progress
	let invoke_action_clone = invoke_action.clone();
	let callback = move |machine_description: &str| {
		// do we need to update
		if throttle.accept().is_ok() {
			let machine_description = machine_description.to_string();
			let command = Action::InfoDbBuildProgress { machine_description };
			invoke_action_clone(command, true);
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
	invoke_action(Action::InfoDbBuildComplete, false);

	// and return the result
	result
}

fn make_invoke_action(
	callback_bubble: ThreadLocalBubble<ActionCallback>,
	canceller: Canceller,
) -> impl Fn(Action, bool) + Clone {
	// lambda to invoke a command on the main event loop; there is some nontrivial stuff here
	// because of the need to put the callback in the "bubble" as well as to ensure that we
	// don't invoke the command if the user cancelled
	move |action, silent_when_cancelled| {
		let callback_bubble = callback_bubble.clone();
		let canceller = canceller.clone();
		invoke_from_event_loop(move || {
			if !silent_when_cancelled || canceller.status().is_continue() {
				(callback_bubble.unwrap())(action);
			}
		})
		.unwrap();
	}
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

impl From<&PreflightProblem> for PreflightFailureReportProblem {
	fn from(problem: &PreflightProblem) -> Self {
		let text = problem.to_shared_string();
		let path_type = problem
			.problem_type()
			.map(|path_type| path_type.to_shared_string())
			.unwrap_or_default();
		let action = problem
			.problem_type()
			.map(|path_type| Action::SettingsPaths(Some(path_type)));
		let action = action.map(|action| action.encode_for_slint()).unwrap_or_default();
		Self {
			text,
			path_type,
			action,
		}
	}
}
