use anyhow::Result;
use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::Window;
use winit::window::WindowAttributes;

use crate::childwindow::ChildWindowImpl;

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

		#[cfg(target_os = "windows")]
		winit::platform::windows::WindowExtWindows::set_enable(&window, false);

		Ok(Self(window))
	}
}

impl ChildWindowImpl for WinitChildWindow {
	fn set_active(&self, is_visible: bool) {
		self.0.set_visible(is_visible);

		#[cfg(target_os = "windows")]
		winit::platform::windows::WindowExtWindows::set_enable(&self.0, is_visible);
	}

	fn update(&self, position: dpi::PhysicalPosition<u32>, size: dpi::PhysicalSize<u32>) {
		self.0.set_outer_position(position);
		let _ = self.0.request_inner_size(size);
	}

	fn text(&self) -> String {
		let raw_window_handle = self.0.window_handle().unwrap().as_raw();
		handle_text(&raw_window_handle).unwrap()
	}

	fn ensure_proper_focus(&self) {
		#[cfg(target_family = "windows")]
		if let RawWindowHandle::Win32(win32_window_handle) = self.0.window_handle().unwrap().as_raw() {
			use tracing::debug;
			use windows::Win32::Foundation::HWND;
			use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
			use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
			use windows::Win32::UI::WindowsAndMessaging::GetParent;

			let is_visible = self.0.is_visible().unwrap_or_default();
			let child_hwnd = HWND(win32_window_handle.hwnd.get() as *mut std::ffi::c_void);
			let parent_hwnd = unsafe { GetParent(child_hwnd) };
			let focus_hwnd = unsafe { GetFocus() };

			let set_focus_hwnd = if is_visible {
				(Ok(focus_hwnd) == parent_hwnd).then_some(child_hwnd)
			} else {
				(focus_hwnd == child_hwnd).then_some(parent_hwnd.clone().ok()).flatten()
			};

			debug!(parent_hwnd=?parent_hwnd, child_hwnd=?child_hwnd, focus_hwnd=?focus_hwnd, is_visible=?is_visible, set_focus_hwnd=?set_focus_hwnd, "ensure_proper_focus()");

			if let Some(set_focus_hwnd) = set_focus_hwnd {
				let _ = unsafe { SetFocus(Some(set_focus_hwnd)) };
			}
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
