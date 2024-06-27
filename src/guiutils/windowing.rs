use i_slint_backend_winit::winit::platform::windows::WindowExtWindows;
use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::ComponentHandle;
use slint::Weak;

use super::MODAL_PARENT;

/// Very hackish function to hook into window creation to create a window as
pub fn with_modal_parent<T>(
	parent: &(impl ComponentHandle + 'static),
	func: impl FnOnce() -> T,
) -> T
where
	T: ComponentHandle,
{
	parent
		.window()
		.with_winit_window(|window| window.set_enable(false));
	let raw_parent = parent
		.window()
		.window_handle()
		.window_handle()
		.unwrap()
		.as_raw();
	match raw_parent {
		#[cfg(target_os = "windows")]
		RawWindowHandle::Win32(win32_window) => MODAL_PARENT.set(Some(win32_window)),

		_ => {
			// Do nothing
		}
	}

	let result = func();
	MODAL_PARENT.set(None);

	let window_reenabler = WindowReenabler {
		weak: parent.as_weak(),
	};
	result
		.window()
		.set_rendering_notifier(move |_, _| window_reenabler.dummy())
		.unwrap();

	result
}

struct WindowReenabler<T>
where
	T: ComponentHandle,
{
	weak: Weak<T>,
}

impl<T> WindowReenabler<T>
where
	T: ComponentHandle,
{
	pub fn dummy(&self) {}
}

impl<T> Drop for WindowReenabler<T>
where
	T: ComponentHandle,
{
	fn drop(&mut self) {
		self.weak
			.unwrap()
			.window()
			.with_winit_window(|window| window.set_enable(true));
	}
}
