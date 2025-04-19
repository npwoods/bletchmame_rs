use anyhow::Result;
use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::PhysicalPosition;
use slint::PhysicalSize;
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
	#[error("cannot create child window")]
	CannotCreateChildWindow,
}

pub struct ChildWindow(ChildWindowInternal);

enum ChildWindowInternal {
	Winit(winit::window::Window),

	#[cfg(feature = "slint-qt-backend")]
	Qt(std::rc::Rc<dyn slint::platform::WindowAdapter>),
}

impl ChildWindow {
	pub fn new(parent: &Window) -> Result<Self> {
		let result = if parent.with_winit_window(|_| ()).is_some() {
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
			ChildWindowInternal::Winit(window)
		} else {
			#[cfg(feature = "slint-qt-backend")]
			{
				// we're using the Qt back end; create a WindowAdapter
				let window_adapter = parent.create_child_window()?;
				window_adapter.qt_win_id().ok_or(ThisError::CannotCreateChildWindow)?;
				ChildWindowInternal::Qt(window_adapter)
			}

			#[cfg(not(feature = "slint-qt-backend"))]
			return Err(ThisError::CannotCreateChildWindow.into());
		};
		Ok(Self(result))
	}

	pub fn set_visible(&self, is_visible: bool) {
		match &self.0 {
			ChildWindowInternal::Winit(window) => window.set_visible(is_visible),
			#[cfg(feature = "slint-qt-backend")]
			ChildWindowInternal::Qt(window_adapter) => window_adapter.set_visible(is_visible).unwrap(),
		}
	}

	pub fn update(&self, container: &Window, top: f32) {
		let position = PhysicalPosition {
			x: 0,
			y: (top * container.scale_factor()) as i32,
		};
		let size = container.size();
		let size = PhysicalSize::new(size.width, size.height - (position.y as u32));
		event!(LOG, position=?position, size=?size, "ChildWindow::update()");

		match &self.0 {
			ChildWindowInternal::Winit(window) => {
				let position = dpi::PhysicalPosition::new(position.x, position.y);
				let size = dpi::PhysicalSize::new(size.width, size.height);
				window.set_outer_position(position);
				let _ = window.request_inner_size(size);

				// hackish (and platform specific) method to "ensure" focus
				container.ensure_child_focus(window);
			}

			#[cfg(feature = "slint-qt-backend")]
			ChildWindowInternal::Qt(window_adapter) => {
				window_adapter.set_position(position.into());
				window_adapter.set_size(slint::WindowSize::Physical(size));
			}
		}
	}

	pub fn text(&self) -> String {
		match &self.0 {
			ChildWindowInternal::Winit(window) => {
				let raw_window_handle = window.window_handle().unwrap().as_raw();
				handle_text(&raw_window_handle).unwrap()
			}

			#[cfg(feature = "slint-qt-backend")]
			ChildWindowInternal::Qt(window_adapter) => window_adapter.qt_win_id().unwrap().to_string(),
		}
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
