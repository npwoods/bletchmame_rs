use std::future::Future;
use std::rc::Rc;

use i_slint_backend_winit::winit::platform::windows::WindowExtWindows;
use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::PhysicalPosition;
use slint::Window;
use slint::WindowHandle;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::WindowAttributes;

use crate::guiutils::hook::with_attributes_hook;

pub struct Modal<D> {
	reenable_parent: Rc<dyn Fn() + 'static>,
	dialog: D,
}

impl<D> Modal<D>
where
	D: ComponentHandle + 'static,
{
	pub fn new(parent: &(impl ComponentHandle + 'static), func: impl FnOnce() -> D) -> Self {
		// disable the parent and get the parent handle and position
		let (parent_handle, parent_position) = {
			let window = parent.window();
			window.with_winit_window(|window| window.set_enable(false));
			(window.window_handle().clone(), window.position())
		};

		// set up a hook
		let hook = move |window_attributes| {
			set_window_attributes_for_modal_parent(window_attributes, &parent_handle, parent_position)
		};

		// invoke the func
		let dialog = with_attributes_hook(func, hook);

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
		}
	}

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

fn set_window_attributes_for_modal_parent(
	mut window_attributes: WindowAttributes,
	parent_handle: &WindowHandle,
	parent_position: PhysicalPosition,
) -> WindowAttributes {
	match parent_handle.window_handle().unwrap().as_raw() {
		// modal dialog on Windows
		#[cfg(target_os = "windows")]
		RawWindowHandle::Win32(win32_window) => {
			let position = winit::dpi::PhysicalPosition {
				x: parent_position.x + 64,
				y: parent_position.y + 64,
			};
			window_attributes = window_attributes.with_owner_window(win32_window.hwnd.into());
			window_attributes.position = Some(position.into());
		}

		// no modal dialog, or unknown platform
		_ => {}
	};

	window_attributes
}

fn reenable_modal_parent(parent: &impl ComponentHandle) {
	parent.window().with_winit_window(|window| window.set_enable(true));
}
