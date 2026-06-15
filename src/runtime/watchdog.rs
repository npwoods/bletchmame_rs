use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::sync::mpsc::channel;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use sysinfo::Pid;
use sysinfo::ProcessesToUpdate;
use sysinfo::System;
use tracing::error;
use tracing::warn;

#[derive(Clone, Copy, Debug, PartialEq)]
enum WatchdogState {
	Idle,
	Wait,
}

pub struct Watchdog {
	sender: Sender<Option<WatchdogState>>,
	thread: Option<JoinHandle<()>>,
	timed_out: Arc<AtomicBool>,
}

impl Watchdog {
	pub fn new(pid: u32, timeout: Duration) -> Self {
		let (sender, receiver) = channel();
		let timed_out = Arc::new(AtomicBool::new(false));
		let timed_out_clone = Arc::clone(&timed_out);
		let thread = thread::spawn(move || thread_proc(receiver, pid, timeout, timed_out_clone));
		let thread = Some(thread);
		Self {
			sender,
			thread,
			timed_out,
		}
	}

	pub fn with_timeout<T>(&self, f: impl FnOnce() -> T) -> (T, bool) {
		self.send_state(Some(WatchdogState::Wait));
		let result = f();
		self.send_state(Some(WatchdogState::Idle));
		let timed_out = self.timed_out.load(Ordering::SeqCst);
		(result, timed_out)
	}

	fn send_state(&self, state: Option<WatchdogState>) {
		let _ = self.sender.send(state);
	}
}

impl Drop for Watchdog {
	fn drop(&mut self) {
		self.send_state(None);
		self.thread
			.take()
			.unwrap()
			.join()
			.expect("Failed to join watchdog thread");
	}
}

fn thread_proc(receiver: Receiver<Option<WatchdogState>>, pid: u32, timeout: Duration, timed_out: Arc<AtomicBool>) {
	let mut sys = System::new_all();
	sys.refresh_processes(ProcessesToUpdate::All, true);
	let Some(process) = sys.process(Pid::from_u32(pid)) else {
		warn!("Process with PID {} not found; watchdog will not be functional", pid);
		return;
	};

	let mut state = Some(WatchdogState::Idle);
	while let Some(current_state) = state {
		let result = match current_state {
			WatchdogState::Wait => receiver.recv_timeout(timeout),
			WatchdogState::Idle => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
		};

		state = match result {
			Ok(new_state) => new_state,
			Err(RecvTimeoutError::Timeout) => {
				error!("Watchdog timeout for MAME process with PID {}", pid);
				timed_out.store(true, Ordering::SeqCst);
				process.kill();
				None
			}
			Err(RecvTimeoutError::Disconnected) => None,
		};
	}
}
