use std::fmt::Display;
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
use slint::invoke_from_event_loop;
use smol_str::SmolStr;
use smol_str::ToSmolStr;
use smol_str::format_smolstr;
use strum::EnumProperty;
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
		start_args: Option<Box<MameStartArgs>>,
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
		start_args: Box<MameStartArgs>,
		current_asset_name: Option<SmolStr>,
		current_progress: f32,
	},
}

#[derive(Debug, EnumProperty)]
enum Failure {
	#[strum(props(Message = "BletchMAME requires additional configuration in order to properly interface with MAME"))]
	Preflight(Box<[PreflightProblem]>),

	#[strum(props(Message = "MAME has errored and shut down"))]
	SessionError(SessionError),

	#[strum(props(Submessage = "This is a very unexpected internal error"))]
	InfoDbStatusMismatch {
		status_build: MameVersion,
		infodb_build: MameVersion,
	},

	#[strum(props(Message = "Status update from MAME is incorrect"))]
	InvalidStatusUpdate(Vec<UpdateXmlProblem>),

	#[strum(props(Message = "Failure processing machine information from MAME"))]
	InfoDbBuild(Error),

	#[strum(props(Message = "Processing machine information from MAME was cancelled"))]
	InfoDbBuildCancelled,

	#[strum(props(Message = "Audit failure before run"))]
	AuditResults(Box<[(Asset, AuditResult)]>),

	#[strum(props(Message = "Unexpected error auditing before run"))]
	AuditError(Error),

	#[strum(props(Message = "Audit was cancelled"))]
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

#[derive(Default, Debug)]
pub struct Report {
	pub message: SmolStr,
	pub submessage: Option<SmolStr>,
	pub mame_stderr_output: Option<SmolStr>,
	pub mame_exit_code: Option<i32>,
	pub button: Option<Button>,
	pub spinner_progress: Option<f32>,
	pub issues: Vec<Issue>,
	pub audit_results: Box<[(Asset, AuditResult)]>,
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

	fn start_session(&self, mame_args: MameArguments, start_args: Option<Box<MameStartArgs>>) -> Session {
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
		let video = start_args.and_then(|x| x.video);
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

	pub fn start(&mut self, start_args: impl Into<Box<MameStartArgs>>) -> bool {
		let start_args = start_args.into();
		info!(?start_args, "AppState::start()");

		// access the InfoDb now
		let info_db = self.info_db().unwrap().clone();

		// access the live session (which had better be present)
		let session = self.live.as_mut().unwrap().session.as_mut().unwrap();

		// do we have an active session that with the expected video state?
		if let SessionState::Active { active_state, .. } = &mut session.session_state
			&& session.video == start_args.video
		{
			// there is indeed - start an auditing session
			let rom_paths = self.preferences.paths.roms.clone();
			let sample_paths = self.preferences.paths.samples.clone();
			let callback = self.fixed.callback.clone();
			let job = match spawn_audit(info_db, rom_paths, sample_paths, AUDIT_DELAY, &start_args, callback) {
				Ok(job) => job,
				Err(e) => {
					self.failure = Some(Failure::AuditError(e));
					return true;
				}
			};
			*active_state = SessionActiveState::Auditing {
				job,
				start_args,
				current_asset_name: None,
				current_progress: 0.0,
			};
		} else {
			// the session is unusable either because it is shutting down and/or the video is wrong; we need to restart
			let start_args = Some(start_args);
			session.session_state = SessionState::Restarting { start_args };

			// cancel the current session
			session.job.cancel();
		};
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
				// the audit succeeded and we're ready to go; build the command that will ultimately
				// be issued to MAME/worker_ui
				let command = MameCommand::start(&start_args);

				// dispatch the command
				command_sender.send(command).unwrap();

				// and set the state to "starting"
				*active_state = SessionActiveState::EmuStarting;
			}
			AuditJobResult::Cancelled => {
				self.failure = Some(Failure::AuditCancelled);
			}
			AuditJobResult::Failed(items) => {
				self.failure = Some(Failure::AuditResults(items));
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
					.map(|running| live.info_db.machines().find(&running.machine_name).unwrap().name())
			})
			.unwrap_or_default()
	}

	pub fn report(&self) -> Option<Report> {
		#[derive(Debug)]
		enum ReportType<'a> {
			// session related reports
			SessionStarting,
			SessionRestarting,
			SessionRestartingForEmu,
			SessionShuttingDown,

			// messages for an emulation starting up or shutting down
			EmuStarting,
			EmuStopping,
			Auditing(Option<&'a SmolStr>, f32),

			// InfoDb building
			InfoDbBuild(Option<&'a str>),

			// failure reports
			FailureReport(&'a Failure),
		}

		// lots of gnarly logic here
		let report_type = if let Some(info_db_build) = self.info_db_build.as_ref() {
			// report that we have an active InfoDb build
			Some(ReportType::InfoDbBuild(info_db_build.machine_description.as_deref()))
		} else if let Some(failure) = self.failure.as_ref() {
			// report that something out there failed
			Some(ReportType::FailureReport(failure))
		} else if let Some(session) = self.live.as_ref().and_then(|live| live.session.as_ref()) {
			match &session.session_state {
				SessionState::ShuttingDown => Some(ReportType::SessionShuttingDown),
				SessionState::Stopping => None,
				SessionState::Restarting { start_args, .. } => {
					if start_args.is_some() {
						Some(ReportType::SessionRestartingForEmu)
					} else {
						Some(ReportType::SessionRestarting)
					}
				}
				SessionState::Active { active_state, .. } => match active_state {
					SessionActiveState::Normal => session.status.is_none().then_some(ReportType::SessionStarting),
					SessionActiveState::EmuStarting => Some(ReportType::EmuStarting),
					SessionActiveState::EmuStopping => Some(ReportType::EmuStopping),
					SessionActiveState::Auditing {
						current_asset_name,
						current_progress,
						..
					} => Some(ReportType::Auditing(current_asset_name.as_ref(), *current_progress)),
				},
			}
		} else {
			None
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
					spinner_progress: Some(f32::NAN),
					..Default::default()
				}
			}
			ReportType::SessionRestarting => Report {
				message: "Resetting MAME...".into(),
				spinner_progress: Some(f32::NAN),
				..Default::default()
			},
			ReportType::SessionRestartingForEmu => Report {
				message: "Starting emulation...".into(),
				submessage: Some("MAME needs to be reset to run this emulation".into()),
				spinner_progress: Some(f32::NAN),
				..Default::default()
			},
			ReportType::SessionStarting => Report {
				message: "Starting MAME...".into(),
				submessage: Some("Waiting for emulation startup to be complete".into()),
				spinner_progress: Some(f32::NAN),
				..Default::default()
			},
			ReportType::SessionShuttingDown => Report {
				message: "MAME is shutting down...".into(),
				spinner_progress: Some(f32::NAN),
				..Default::default()
			},
			ReportType::EmuStarting => Report {
				message: "Starting emulation...".into(),
				spinner_progress: Some(f32::NAN),
				..Default::default()
			},
			ReportType::EmuStopping => Report {
				message: "Stopping emulation...".into(),
				spinner_progress: Some(f32::NAN),
				..Default::default()
			},
			ReportType::Auditing(current_asset_name, current_progress) => {
				let button = Button {
					text: "Cancel".into(),
					command: Action::AuditCancel,
				};
				Report {
					message: "Auditing assets...".into(),
					submessage: current_asset_name.cloned(),
					spinner_progress: Some(current_progress),
					button: Some(button),
					..Default::default()
				}
			}
			ReportType::FailureReport(failure) => failure.report(),
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

impl Failure {
	pub fn report(&self) -> Report {
		// primary message
		let message = if let Self::InfoDbStatusMismatch {
			status_build,
			infodb_build,
		} = self
		{
			format_smolstr!(
				"The MAME Status Update is reporting version {status_build} and the MAME Machine Info output is reporting version {infodb_build}"
			)
		} else {
			self.get_str("Message").unwrap().into()
		};

		// secondary message
		let submessage: Option<&dyn Display> = match self {
			Self::SessionError(error) => Some(error),
			Self::InfoDbBuild(error) | Self::AuditError(error) => Some(error),
			_ => None,
		};
		let submessage = submessage
			.map(|x| format_smolstr!("{x}"))
			.or_else(|| self.get_str("Submessage").map(SmolStr::new_static));

		// issues
		let issues = match self {
			Self::Preflight(preflight_problems) => preflight_problems
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
				.collect(),

			Self::InvalidStatusUpdate(errors) => errors
				.iter()
				.map(|e| Issue {
					text: format!("{e}").into(),
					button: None,
				})
				.collect(),
			_ => Default::default(),
		};

		// MAME error output and exit code
		let (mame_stderr_output, mame_exit_code) = if let Failure::SessionError(error) = self {
			(error.mame_stderr_text.clone(), error.exit_code)
		} else {
			(None, None)
		};

		// action button
		let button = match self {
			Self::SessionError(_) | Self::AuditResults(_) | Self::AuditError(_) | Self::AuditCancelled => {
				Some(Button {
					text: "Continue".into(),
					command: Action::ReactivateMame,
				})
			}
			Self::InfoDbBuild(_) => Some(Button {
				text: "Retry".into(),
				command: Action::HelpRefreshInfoDb,
			}),
			Self::InfoDbStatusMismatch { .. } => Some(Button {
				text: "Retry".into(),
				command: Action::ReactivateMame,
			}),
			_ => None,
		};

		// auditing results
		let audit_results = if let Self::AuditResults(audit_results) = self {
			audit_results.clone()
		} else {
			Default::default()
		};

		Report {
			message,
			submessage,
			issues,
			mame_stderr_output,
			mame_exit_code,
			button,
			spinner_progress: None,
			audit_results,
		}
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
	audit_delay: Option<Duration>,
	start_args: &MameStartArgs,
	callback: ActionCallback,
) -> Result<Job<AuditJobResult>> {
	// create the MachineConfig
	let machine_config = MachineConfig::from_mame_start_args(info_db.clone(), start_args)?;

	// create the assets
	let assets = Asset::from_machine_config(&machine_config);

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
