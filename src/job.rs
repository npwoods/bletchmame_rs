use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::thread::spawn;

/// Encapsulation of a "job"; a task spawned into a separate thread with a facility for cancellation
#[derive(Debug)]
pub struct Job<T> {
	join_handle: JoinHandle<T>,
	canceller: Canceller,
}

#[derive(Clone, Debug, Default)]
pub struct Canceller(Arc<AtomicBool>);

impl<T> Job<T>
where
	T: Send + 'static,
{
	pub fn new(f: impl FnOnce(Canceller) -> T + Send + 'static) -> Self {
		let canceller = Canceller::default();
		let canceller_clone = canceller.clone();
		let join_handle = spawn(|| f(canceller_clone));
		Self { join_handle, canceller }
	}

	pub fn join(self) -> T {
		// in practice we cannot meaningfully recover from JoinHandle::join() failing, so
		// we do an expect
		self.join_handle.join().expect("join failed")
	}

	pub fn cancel(&self) {
		self.canceller.0.store(true, Ordering::Relaxed);
	}
}

impl Canceller {
	pub fn status(&self) -> ControlFlow<()> {
		if self.0.load(Ordering::Relaxed) {
			ControlFlow::Break(())
		} else {
			ControlFlow::Continue(())
		}
	}
}
