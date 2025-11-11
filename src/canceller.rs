use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

#[derive(Clone, Debug, Default)]
pub struct Canceller(Arc<AtomicBool>);

impl Canceller {
	pub fn cancel(&self) {
		self.0.store(true, Ordering::Relaxed);
	}

	pub fn status(&self) -> ControlFlow<()> {
		if self.0.load(Ordering::Relaxed) {
			ControlFlow::Break(())
		} else {
			ControlFlow::Continue(())
		}
	}
}
