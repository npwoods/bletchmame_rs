use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::marker::PhantomData;
use std::sync::Arc;
use std::thread;
use std::thread::ThreadId;

type Id = usize;

#[derive(Clone, Debug)]
pub struct ThreadLocalBubble<T>
where
	T: 'static,
{
	inner: Arc<Inner>,
	phantom: PhantomData<fn() -> T>,
}

impl<T> ThreadLocalBubble<T>
where
	T: 'static,
{
	pub fn new(inner: T) -> Self {
		let id = LOCAL.with_borrow_mut(|local_data| {
			// replace this code when `Option::get_or_insert_default()` stabilizes
			if local_data.map.is_none() {
				local_data.map = Some(HashMap::new());
			}
			let map = local_data.map.as_mut().unwrap();

			loop {
				local_data.counter += 1;

				let entry = map.entry(local_data.counter);
				if matches!(entry, Entry::Vacant(_)) {
					entry.or_insert(Box::new(inner) as Box<dyn Any + 'static>);
					break local_data.counter;
				}
			}
		});

		let owning_thread = thread::current().id();
		let inner = Inner { id, owning_thread };
		let inner = Arc::new(inner);
		Self {
			inner,
			phantom: PhantomData,
		}
	}

	pub fn with_borrow<R>(&self, f: impl Fn(&T) -> R) -> Option<R> {
		(self.inner.owning_thread == thread::current().id()).then(|| {
			let id = self.inner.id;
			LOCAL.with_borrow(|local_data| {
				let any = local_data.map.as_ref().unwrap().get(&id).unwrap().as_ref();
				f(any.downcast_ref::<T>().unwrap())
			})
		})
	}
}
impl<T> ThreadLocalBubble<T>
where
	T: 'static + Clone,
{
	pub fn unwrap(&self) -> T {
		self.try_unwrap().unwrap()
	}

	pub fn try_unwrap(&self) -> Option<T> {
		self.with_borrow(|x| x.clone())
	}
}

#[derive(Debug)]
struct Inner {
	id: Id,
	owning_thread: ThreadId,
}

impl Drop for Inner {
	fn drop(&mut self) {
		if self.owning_thread == thread::current().id() {
			let _ = LOCAL.try_with(|local_data| {
				let mut local_data = local_data.borrow_mut();
				if let Some(map) = local_data.map.as_mut() {
					map.remove(&self.id);
				};
				if local_data.map.as_ref().is_some_and(|x| x.is_empty()) {
					local_data.map = None;
				}
			});
		}
	}
}

#[derive(Debug, Default)]
struct ThreadLocalData {
	counter: Id,
	map: Option<HashMap<usize, Box<dyn Any + 'static>>>,
}

thread_local! {
	pub static LOCAL: RefCell<ThreadLocalData> = RefCell::new(Default::default())
}

#[cfg(test)]
mod test {
	use std::rc::Rc;
	use std::thread::spawn;

	use super::LOCAL;
	use super::ThreadLocalBubble;

	#[test]
	pub fn test() {
		// `Rc` is something that cannot be sent across threads
		let rc = Rc::new("Hello");
		let bubble = ThreadLocalBubble::new(rc);

		// push the bubble into a different thread and touch it
		let thread = spawn(move || {
			let _ = format!("{:?}", &bubble);
			bubble.clone()
		});

		// get the data back out of the thread and validate the bubble worked
		let returned_bubble = thread.join().unwrap();
		assert_eq!("Hello", *returned_bubble.unwrap());

		// verify that dropping the bubble causes the local data HashMap to go away
		drop(returned_bubble);
		LOCAL.with_borrow(|local_data| {
			assert!(local_data.map.is_none());
		});
	}
}
