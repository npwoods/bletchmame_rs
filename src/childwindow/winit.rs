use anyhow::Result;
use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::Window;
use winit::window::WindowAttributes;

use crate::childwindow::ChildWindowImpl;
use crate::platform::WindowExt;

pub struct WinitChildWindow(winit::window::Window);

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("unknown raw handle type")]
	UnknownRawHandleType,
}

impl WinitChildWindow {
	pub fn new(parent: &Window) -> Result<Self> {
		// we're using the winit backend; access the raw hande for the parent - if
		// we can't access the so-called "handle text", we can't use the window and we return an error
		let raw_window_handle = parent.window_handle().window_handle()?.as_raw();
		handle_text(&raw_window_handle)?;

		let window_attributes = unsafe {
			WindowAttributes::default()
				.with_title("MAME Child Window")
				.with_visible(false)
				.with_decorations(false)
				.with_parent_window(Some(raw_window_handle))
		};

		let window = parent.create_winit_window(window_attributes)?;
		Ok(Self(window))
	}
}

impl ChildWindowImpl for WinitChildWindow {
	fn set_visible(&self, is_visible: bool) {
		self.0.set_visible(is_visible);
	}

	fn update(&self, position: dpi::PhysicalPosition<u32>, size: dpi::PhysicalSize<u32>) {
		self.0.set_outer_position(position);
		let _ = self.0.request_inner_size(size);
	}

	fn text(&self) -> String {
		let raw_window_handle = self.0.window_handle().unwrap().as_raw();
		handle_text(&raw_window_handle).unwrap()
	}

	fn ensure_child_focus(&self, container: &Window) {
		container.ensure_child_focus(&self.0);
	}
}

fn handle_text(raw_window_handle: &RawWindowHandle) -> Result<String> {
	match raw_window_handle {
		#[cfg(target_family = "windows")]
		RawWindowHandle::Win32(win32_window_handle) => Ok(win32_window_handle.hwnd.to_string()),

		#[cfg(target_family = "unix")]
		RawWindowHandle::Xlib(xlib_window_handle) => Ok(xlib_window_handle.window.to_string()),

		_ => Err(ThisError::UnknownRawHandleType.into()),
	}
}
