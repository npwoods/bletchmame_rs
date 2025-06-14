use std::cell::RefCell;
use std::rc::Rc;

use tokio::sync::Notify;

pub mod configure;
pub mod devimages;
pub mod file;
pub mod image;
pub mod input;
pub mod input_multi;
pub mod messagebox;
pub mod namecollection;
pub mod paths;
pub mod seqpoll;
pub mod socket;

struct SingleResult<T>(Rc<(Notify, RefCell<Option<T>>)>);

impl<T> SingleResult<T> {
	pub async fn wait(self) -> T {
		let (notify, cell) = self.0.as_ref();
		notify.notified().await;
		cell.borrow_mut().take().unwrap()
	}

	pub fn signaller(&self) -> SingleResultSignaller<T> {
		SingleResultSignaller(self.0.clone())
	}
}

impl<T> Default for SingleResult<T> {
	fn default() -> Self {
		let result = (Notify::new(), RefCell::new(None));
		Self(Rc::new(result))
	}
}

struct SingleResultSignaller<T>(Rc<(Notify, RefCell<Option<T>>)>);

impl<T> SingleResultSignaller<T> {
	pub fn signal(&self, result: T) {
		let (notify, cell) = self.0.as_ref();
		cell.replace(Some(result));
		notify.notify_one();
	}
}
