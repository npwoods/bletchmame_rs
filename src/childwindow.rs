use anyhow::Result;
use dpi::PhysicalPosition;
use i_slint_backend_winit::WinitWindowAccessor;
use muda::dpi::PhysicalSize;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::Window;
use tracing::Level;
use tracing::event;
use winit::window::WindowAttributes;

use crate::platform::WindowExt;

const LOG: Level = Level::DEBUG;

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("unknown raw handle type")]
	UnknownRawHandleType,
}

pub struct ChildWindow(winit::window::Window);

impl ChildWindow {
	pub fn new(parent: &Window) -> Result<Self> {
		// access the raw hande for the parent - if we can't access the so-called "handle text", we
		// can't use the window and we return an error
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

	pub fn set_visible(&self, is_visible: bool) {
		self.0.set_visible(is_visible);
	}

	pub fn update(&self, container: &Window, top: f32) {
		// determine position and size
		let position = PhysicalPosition::new(0, (top * container.scale_factor()) as u32);
		let size = container.size();
		let size = PhysicalSize::new(size.width, size.height - position.y);
		event!(LOG, position=?position, size=?size, "ChildWindow::update()");

		// and set them
		self.0.set_outer_position(position);
		let _ = self.0.request_inner_size(size);

		// hackish (and platform specific) method to "ensure" focus
		container.ensure_child_focus(&self.0);
	}

	pub fn text(&self) -> String {
		let raw_window_handle = self.0.window_handle().unwrap().as_raw();
		handle_text(&raw_window_handle).unwrap()
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
