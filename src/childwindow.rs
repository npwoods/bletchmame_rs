use anyhow::Result;
use dpi::PhysicalPosition;
use i_slint_backend_winit::create_winit_window;
use muda::dpi::PhysicalSize;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::Window;
use tracing::event;
use tracing::Level;
use winit::window::WindowAttributes;

use crate::platform::WindowExt;

const LOG: Level = Level::TRACE;

pub struct ChildWindow(Option<winit::window::Window>);

impl ChildWindow {
	pub fn new(parent: &Window) -> Result<Self> {
		// access the raw hande for the parent - if we can't access the so-called "handle text", we
		// can't use the window and we return a bogus child window
		let raw_window_handle = parent.window_handle().window_handle()?.as_raw();
		if handle_text(&raw_window_handle).is_none() {
			return Ok(Self(None));
		}

		let window_attributes = unsafe {
			WindowAttributes::default()
				.with_title("MAME Child Window")
				.with_visible(false)
				.with_decorations(false)
				.with_parent_window(Some(raw_window_handle))
		};

		let window = create_winit_window(window_attributes)?;
		Ok(Self(Some(window)))
	}

	pub fn set_visible(&self, is_visible: bool) {
		let Some(window) = &self.0 else {
			return;
		};
		window.set_visible(is_visible);
	}

	pub fn update(&self, container: &Window, top: f32) {
		let Some(window) = &self.0 else {
			return;
		};

		// determine position and size
		let position = PhysicalPosition::new(0, (top * container.scale_factor()) as u32);
		let size = container.size();
		let size = PhysicalSize::new(size.width, size.height - position.y);
		event!(LOG, "ChildWindow::update(): position={:?} size={:?}", position, size);

		// and set them
		window.set_outer_position(position);
		let _ = window.request_inner_size(size);

		// hackish (and platform specific) method to "ensure" focus
		container.ensure_child_focus(window);
	}

	pub fn text(&self) -> Option<String> {
		let window = self.0.as_ref()?;
		let raw_window_handle = window.window_handle().unwrap().as_raw();
		let text = handle_text(&raw_window_handle).expect("Can't identify handle type");
		Some(text)
	}
}

fn handle_text(raw_window_handle: &RawWindowHandle) -> Option<String> {
	match raw_window_handle {
		#[cfg(target_family = "windows")]
		RawWindowHandle::Win32(win32_window_handle) => Some(win32_window_handle.hwnd.to_string()),

		#[cfg(target_family = "unix")]
		RawWindowHandle::Xlib(xlib_window_handle) => Some(xlib_window_handle.window.to_string()),

		_ => None,
	}
}
