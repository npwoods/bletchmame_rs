use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use i_slint_backend_winit::WinitWindowAccessor;
use i_slint_backend_winit::WinitWindowEventResult;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::PhysicalPosition;
use slint::Window;
use slint::WindowHandle;
use winit::event::WindowEvent;

use crate::backend::BackendRuntime;
use crate::guiutils::component::ComponentWrap;
use crate::guiutils::component::WeakComponentWrap;
use crate::platform::WindowExt;

#[derive(Clone)]
pub struct ModalStack {
	backend_runtime: BackendRuntime,
	stack: Rc<RefCell<Vec<WeakComponentWrap>>>,
}

pub struct Modal<D> {
	modal_stack: ModalStack,
	modal_stack_pos: usize,
	reenable_parent: Rc<dyn Fn() + 'static>,
	dialog: D,
}

impl ModalStack {
	pub fn new(backend_runtime: BackendRuntime, window: &(impl ComponentHandle + Sized + 'static)) -> Self {
		let vec: Vec<WeakComponentWrap> = vec![WeakComponentWrap::from(window)];
		let stack = Rc::new(RefCell::new(vec));
		Self { backend_runtime, stack }
	}

	pub fn modal<D>(&self, func: impl FnOnce() -> D) -> Modal<D>
	where
		D: ComponentHandle + 'static,
	{
		let modal_stack_pos = self.stack.borrow().len();
		let parent = self.stack.borrow().last().unwrap().unwrap();

		// disable the parent
		parent.window().set_enabled_for_modal(false);

		// invoke the func
		let dialog = self.backend_runtime.with_modal_parent(parent.window(), func);

		// add this dialog to the stack
		self.stack.borrow_mut().push(dialog.as_weak().into());

		// position the new dialog
		let new_dialog_position = {
			let parent_size = parent.window().size();
			let parent_position = parent.window().position();
			let dialog_size = dialog.window().size();
			let x = parent_position.x
				+ (i32::try_from(parent_size.width).unwrap() - i32::try_from(dialog_size.width).unwrap()) / 2;
			let y = parent_position.y
				+ (i32::try_from(parent_size.height).unwrap() - i32::try_from(dialog_size.height).unwrap()) / 2;
			PhysicalPosition { x, y }
		};
		dialog.window().set_position(new_dialog_position);

		// keep the window on top
		let dialog_weak = dialog.as_weak();
		parent.window().on_winit_window_event(move |_, evt| {
			let dialog = matches!(evt, WindowEvent::Focused(true))
				.then(|| dialog_weak.upgrade())
				.flatten();
			if let Some(dialog) = dialog {
				dialog.window().with_winit_window(|window| window.focus_window());
				WinitWindowEventResult::PreventDefault
			} else {
				WinitWindowEventResult::Propagate
			}
		});

		// set up a bogus callback because the default callback won't do the right thing
		dialog
			.window()
			.on_close_requested(move || panic!("Need to override on_close_requested"));

		// create a callback to reenable the parent
		let parent = parent.clone_strong();
		let reenable_parent = move || reenable_modal_parent(&parent);
		let reenable_parent = Rc::from(reenable_parent);

		// and return
		Modal {
			modal_stack: self.clone(),
			modal_stack_pos,
			reenable_parent,
			dialog,
		}
	}

	pub fn top(&self) -> WindowHandle {
		self.stack.borrow().last().unwrap().unwrap().window().window_handle()
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

	pub fn launch(self) {
		// stow a callback to reenable the parent here
		let reenable_parent_clone = self.reenable_parent.clone();
		self.window().on_close_requested(move || {
			reenable_parent_clone();
			CloseRequestResponse::HideWindow
		});

		// show the dialog
		self.dialog.show().unwrap();
	}

	pub async fn run<R>(self, fut: impl Future<Output = R>) -> R {
		// show the dialog
		self.dialog.show().unwrap();

		// run the function
		let result = fut.await;

		// before we hide the dialog, reenable the parent
		(self.reenable_parent)();

		// hide the dialog
		self.dialog.hide().unwrap();

		// return
		result
	}
}

impl<D> Drop for Modal<D> {
	fn drop(&mut self) {
		// truncate the modal stack
		self.modal_stack.stack.borrow_mut().truncate(self.modal_stack_pos);
	}
}

fn reenable_modal_parent(parent: &ComponentWrap) {
	parent.window().set_enabled_for_modal(true);
	parent
		.window()
		.on_winit_window_event(|_, _| WinitWindowEventResult::Propagate);
}
