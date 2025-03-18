use std::cell::Cell;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use i_slint_backend_winit::WinitWindowAccessor;
use i_slint_backend_winit::WinitWindowEventResult;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::PhysicalPosition;
use slint::Window;
use winit::event::WindowEvent;
use winit::window::WindowAttributes;

use crate::appcommand::AppCommand;
use crate::guiutils::hook::with_attributes_hook;
use crate::platform::WindowAttributesExt;
use crate::platform::WindowExt;

thread_local! {
	#[allow(clippy::type_complexity)]
	static CURRENT_FILTERS: RefCell<Vec<Box<dyn Fn(AppCommand) -> Option<AppCommand> + 'static>>> = RefCell::new(Default::default());
}

pub struct Modal<D> {
	reenable_parent: Rc<dyn Fn() + 'static>,
	dialog: D,
	must_drop_filter: Cell<bool>,
}

impl<D> Modal<D>
where
	D: ComponentHandle + 'static,
{
	pub fn new(parent: &(impl ComponentHandle + 'static), func: impl FnOnce() -> D) -> Self {
		// disable the parent
		parent.window().set_enabled_for_modal(false);

		// set up a hook
		let parent_weak = parent.as_weak();
		let hook = move |window_attributes| {
			set_window_attributes_for_modal_parent(window_attributes, parent_weak.unwrap().window())
		};

		// invoke the func
		let dialog = with_attributes_hook(func, hook);

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
		Self {
			reenable_parent,
			dialog,
			must_drop_filter: Cell::new(false),
		}
	}

	pub fn dialog(&self) -> &'_ D {
		&self.dialog
	}

	pub fn window(&self) -> &'_ Window {
		self.dialog.window()
	}

	pub fn set_command_filter(&self, callback: impl Fn(AppCommand) -> Option<AppCommand> + 'static) {
		let callback = Box::new(callback) as Box<dyn Fn(AppCommand) -> Option<AppCommand> + 'static>;
		CURRENT_FILTERS.with_borrow_mut(|filters| {
			filters.push(callback);
		});
		self.must_drop_filter.set(true);
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
		if self.must_drop_filter.get() {
			CURRENT_FILTERS.with_borrow_mut(|filters| {
				let _ = filters.pop().unwrap();
			});
		}
	}
}

fn set_window_attributes_for_modal_parent(
	mut window_attributes: WindowAttributes,
	parent: &Window,
) -> WindowAttributes {
	window_attributes = window_attributes.with_owner_window(parent);
	window_attributes
}

fn reenable_modal_parent(parent: &impl ComponentHandle) {
	parent.window().set_enabled_for_modal(true);
	parent
		.window()
		.on_winit_window_event(|_, _| WinitWindowEventResult::Propagate);
}

pub fn filter_command(command: AppCommand) -> Option<AppCommand> {
	CURRENT_FILTERS.with_borrow(|filters| {
		if let Some(filter) = filters.last() {
			filter(command)
		} else {
			Some(command)
		}
	})
}
