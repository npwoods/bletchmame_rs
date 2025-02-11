use anyhow::Error;
use anyhow::Result;

use std::cell::RefCell;
use std::rc::Rc;
use std::thread::spawn;
use std::thread::JoinHandle;

#[derive(Debug)]
pub struct Job<T>(Rc<RefCell<Option<JoinHandle<T>>>>);

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
