//! A very simple publish and subscribe channel
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
pub struct Channel<T>(Rc<RefCell<ChannelInner<T>>>);

type Callback<T> = Box<dyn Fn(&T) + 'static>;

#[derive(Default)]
struct ChannelInner<T> {
	subscribers: Vec<Option<Callback<T>>>,
}

struct Subscription<T> {
	id: usize,
	channel: Channel<T>,
}

impl<T> Channel<T> {
	pub fn subscribe(self, callback: impl Fn(&T) + 'static) -> impl Drop {
		let callback = Some(Callback::from(Box::new(callback)));
		let id = {
			let mut inner = self.0.borrow_mut();
			let id = inner.subscribers.iter().position(|x| x.is_none());
			if let Some(id) = id {
				inner.subscribers[id] = callback;
				id
			} else {
				let id = inner.subscribers.len();
				inner.subscribers.push(callback);
				id
			}
		};
		Subscription { id, channel: self }
	}

	pub fn publish(&self, obj: &T) {
		let inner = self.0.borrow();
		for callback in &inner.subscribers {
			if let Some(callback) = callback.as_ref() {
				callback(obj);
			}
		}
	}

	fn unsubscribe(&self, id: usize) {
		let mut inner = self.0.borrow_mut();

		// clear out this subscriber
		inner.subscribers[id] = None;

		// truncate `None` subscribers at the end
		let len = inner
			.subscribers
			.iter()
			.rposition(|x| x.is_some())
			.map(|x| x + 1)
			.unwrap_or(0);
		inner.subscribers.truncate(len);
	}
}

impl<T> Clone for Channel<T> {
	fn clone(&self) -> Self {
		Self(self.0.clone())
	}
}

impl<T> Drop for Subscription<T> {
	fn drop(&mut self) {
		self.channel.unsubscribe(self.id);
	}
}
