use std::cell::RefCell;
use std::sync::Arc;

use tracing::event;
use tracing::Level;

use crate::debugstr::DebugString;
use crate::prefs::PrefsPaths;
use crate::runtime::args::MameArgumentsSource;
use crate::runtime::session::MameSession;
use crate::runtime::MameCommand;
use crate::runtime::MameEvent;
use crate::runtime::MameStderr;
use crate::runtime::MameWindowing;

const LOG: Level = Level::DEBUG;

pub struct MameController {
	session: RefCell<Option<MameSession>>,
	event_callback: RefCell<Arc<dyn Fn(MameEvent) + Send + Sync + 'static>>,
	mame_stderr: MameStderr,
}

impl MameController {
	pub fn new(mame_stderr: MameStderr) -> Self {
		Self {
			session: RefCell::new(None),
			event_callback: RefCell::new(Arc::new(|_| {})),
			mame_stderr,
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
			.is_some_and(|session| !session.has_pending_commands())
	}

	pub fn reset(&self, prefs_paths: Option<&PrefsPaths>, mame_windowing: &MameWindowing) {
		// first and foremost, determine if we actually have enough set up to invoke MAME
		let mame_args: Option<_> = prefs_paths.and_then(|prefs_paths| {
			MameArgumentsSource::new(prefs_paths, mame_windowing)
				.ok()
				.and_then(|x| x.preflight().is_ok().then_some(x))
		});

		// logging
		event!(
			LOG,
			"MameController::reset(): prefs_paths={:?}",
			prefs_paths.as_ref().map(DebugString::elipsis)
		);

		// is there an active session? if so, join it
		if let Some(session) = self.session.take() {
			session.shutdown();
		}

		// are we starting up a new session?
		if let Some(mame_args) = mame_args {
			// we are - start the session
			let event_callback = self.event_callback.borrow().clone();
			let event_callback = move |evt| event_callback(evt);
			let session = MameSession::new(mame_args.into(), event_callback, self.mame_stderr);
			self.session.replace(Some(session));
		}
	}

	pub fn issue_command(&self, command: MameCommand) {
		let session = self.session.borrow();
		let Some(session) = session.as_ref() else {
			event!(LOG, "MameController::issue_command():  No session: {:?}", command);
			return;
		};
		session.issue_command(command);
	}
}
