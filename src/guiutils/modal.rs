use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::PhysicalPosition;
use slint::Window;
use slint::WindowHandle;

use crate::backend::BackendRuntime;
use crate::platform::WindowExt;

#[derive(Clone)]
pub struct ModalStack {
	backend_runtime: BackendRuntime,
	stack: Rc<RefCell<Vec<ModalStackEntry>>>,
}

pub struct Modal<D> {
	bookmark: ModalStackBookmark,
	dialog: D,
}

#[derive(Clone)]
struct ModalStackBookmark {
	modal_stack: ModalStack,
	position: usize,
}

struct ModalStackEntry(Box<dyn Fn() -> Box<dyn WindowOwner>>);

trait WindowOwner {
	fn window(&self) -> &'_ Window;
}

impl ModalStack {
	pub fn new(backend_runtime: BackendRuntime, window: &(impl ComponentHandle + Sized + 'static)) -> Self {
		let vec = vec![ModalStackEntry::new(window)];
		let stack = Rc::new(RefCell::new(vec));
		Self { backend_runtime, stack }
	}

	pub fn modal<D>(&self, func: impl FnOnce() -> D) -> Modal<D>
	where
		D: ComponentHandle + 'static,
	{
		let modal_stack_pos = self.stack.borrow().len();

		let (dialog, parent_size, parent_position) = self.stack.borrow().last().unwrap().with_window(move |parent| {
			// disable the parent
			parent.set_enabled_for_modal(false);

			// invoke the func
			let dialog = self.backend_runtime.with_modal_parent(parent, func);

			// return the dialog, along with size/position
			let parent_size = parent.size();
			let parent_position = parent.position();
			(dialog, parent_size, parent_position)
		});

		// add this dialog to the stack
		self.stack.borrow_mut().push(ModalStackEntry::new(&dialog));

		// position the new dialog
		let new_dialog_position = {
			let dialog_size = dialog.window().size();
			let x = parent_position.x
				+ (i32::try_from(parent_size.width).unwrap() - i32::try_from(dialog_size.width).unwrap()) / 2;
			let y = parent_position.y
				+ (i32::try_from(parent_size.height).unwrap() - i32::try_from(dialog_size.height).unwrap()) / 2;
			PhysicalPosition { x, y }
		};
		dialog.window().set_position(new_dialog_position);

		// set up a bogus callback because the default callback won't do the right thing
		dialog
			.window()
			.on_close_requested(move || panic!("Need to override on_close_requested"));

		// and return
		let bookmark = ModalStackBookmark::new(self.clone(), modal_stack_pos);
		Modal { bookmark, dialog }
	}

	pub fn top(&self) -> WindowHandle {
		self.stack
			.borrow()
			.last()
			.unwrap()
			.with_window(|window: &Window| window.window_handle())
	}
}

impl<D> Modal<D>
where
	D: ComponentHandle + 'static,
{
	pub fn dialog(&self) -> &'_ D {
		&self.dialog
	}

	pub fn window(&self) -> &'_ Window {
		self.dialog.window()
	}

	pub async fn run<R>(self, fut: impl Future<Output = R>) -> R {
		// show the dialog
		self.dialog.show().unwrap();

		// run the function
		let result = fut.await;

		// before we hide the dialog, reenable the parent
		self.bookmark.reenable_parent();

		// hide the dialog
		self.dialog.hide().unwrap();

		// return
		result
	}
}

impl ModalStackBookmark {
	pub fn new(modal_stack: ModalStack, position: usize) -> Self {
		Self { modal_stack, position }
	}

	pub fn reenable_parent(&self) {
		let parent_pos = self.position - 1;
		self.modal_stack.stack.borrow()[parent_pos].with_window(|window| window.set_enabled_for_modal(true));
	}
}

impl Drop for ModalStackBookmark {
	fn drop(&mut self) {
		// truncate the modal stack
		self.modal_stack.stack.borrow_mut().truncate(self.position);
	}
}

impl ModalStackEntry {
	pub fn new<C>(component: &C) -> Self
	where
		C: ComponentHandle + 'static,
	{
		struct MyWindowOwner<C>(C);

		impl<C> WindowOwner for MyWindowOwner<C>
		where
			C: ComponentHandle + 'static,
		{
			fn window(&self) -> &'_ Window {
				self.0.window()
			}
		}

		let component_weak = component.as_weak();
		let func = move || Box::new(MyWindowOwner(component_weak.unwrap())) as Box<dyn WindowOwner>;
		Self(Box::new(func) as Box<_>)
	}

	pub fn with_window<'a, R>(&'a self, callback: impl FnOnce(&Window) -> R) -> R
	where
		R: 'a,
	{
		let window_owner = (self.0)();
		let window = window_owner.window();
		callback(window)
		/*
		let result = Rc::new(RefCell::new(None));
		let result_clone = result.clone();

		self.0(Box::new(move |window| {
			let callback_result = callback(window);
			result_clone.replace(Some(callback_result));
		}));

		Rc::try_unwrap(result)
			.unwrap_or_else(|_| unreachable!("Rc::try_unwrap() failed"))
			.borrow_mut()
			.take()
			.unwrap()
			 */
	}
}
