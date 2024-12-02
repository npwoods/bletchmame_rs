use std::future::Future;

use i_slint_backend_winit::winit::platform::windows::WindowExtWindows;
use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::ComponentHandle;
use slint::PhysicalPosition;
use slint::Window;
use slint::WindowHandle;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::WindowAttributes;

use super::WINDOW_ATTRIBUTE_HOOK_CALLBACK;

/// Very hackish function to hook into window creation to create a window as
pub fn with_modal_parent<T>(parent: &(impl ComponentHandle + 'static), func: impl FnOnce() -> T) -> T
where
	T: ComponentHandle,
{
	// disable the parent and get the parent handle and position
	let (parent_handle, parent_position) = {
		let window = parent.window();
		window.with_winit_window(|window| window.set_enable(false));
		(window.window_handle().clone(), window.position())
	};

	// set up a callback and stow it in WINDOW_ATTRIBUTE_HOOK_CALLBACK
	let callback = move |window_attributes| {
		set_window_attributes_for_modal_parent(window_attributes, &parent_handle, parent_position)
	};
	WINDOW_ATTRIBUTE_HOOK_CALLBACK.set(Some(Box::new(callback)));

	// invoke the callback
	let result = func();

	// force the window to be created
	let _ = result.window().size();

	// clear out WINDOW_ATTRIBUTE_HOOK_CALLBACK
	let old_hook: Option<Box<dyn Fn(WindowAttributes) -> WindowAttributes>> = WINDOW_ATTRIBUTE_HOOK_CALLBACK.take();
	assert!(old_hook.is_some(), "WINDOW_ATTRIBUTE_HOOK_CALLBACK was lost");

	// by default install an `on_close_requested` handle that will reenable the modal parent
	let parent_weak = parent.as_weak();
	result.window().on_close_requested(move || {
		reenable_modal_parent(parent_weak.unwrap().window());
		Default::default()
	});

	result
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

pub async fn run_modal_dialog<R>(
	parent: &impl ComponentHandle,
	dialog: &impl ComponentHandle,
	fut: impl Future<Output = R>,
) -> R {
	// show the dialog
	dialog.show().unwrap();

	// run the function
	let result = fut.await;

	// before we hide the dialog, reenable the parent
	reenable_modal_parent(parent.window());

	// hide the dialog
	dialog.hide().unwrap();

	// return
	result
}

pub fn reenable_modal_parent(parent: &Window) {
	parent.with_winit_window(|window| window.set_enable(true));
}
