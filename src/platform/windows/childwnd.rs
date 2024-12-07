use std::mem::zeroed;
use std::ptr::null;
use std::ptr::null_mut;

use anyhow::Error;
use anyhow::Result;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::PhysicalPosition;
use slint::PhysicalSize;
use slint::Window;
use tracing::event;
use tracing::Level;
use winapi::shared::windef::HWND;
use winapi::um::winuser::CreateWindowExW;
use winapi::um::winuser::GetFocus;
use winapi::um::winuser::GetParent;
use winapi::um::winuser::GetWindowRect;
use winapi::um::winuser::IsWindowVisible;
use winapi::um::winuser::SetFocus;
use winapi::um::winuser::SetWindowPos;
use winapi::um::winuser::ShowWindow;
use winapi::um::winuser::SWP_NOACTIVATE;
use winapi::um::winuser::SWP_NOOWNERZORDER;
use winapi::um::winuser::SWP_NOSENDCHANGING;
use winapi::um::winuser::SW_HIDE;
use winapi::um::winuser::SW_SHOW;
use winapi::um::winuser::WS_CHILD;

const LOG: Level = Level::TRACE;

pub struct WinChildWindow(HWND);

impl WinChildWindow {
	pub fn new(window: &Window) -> Result<Self> {
		let RawWindowHandle::Win32(win32_window) = window.window_handle().window_handle().unwrap().as_raw() else {
			let message = "WinChildWindow::new() - no Win32 window handle";
			return Err(Error::msg(message));
		};

		let class_name = "Static\0".encode_utf16().collect::<Vec<_>>();
		let style = WS_CHILD;
		let ex_style = 0;
		let hwnd = unsafe {
			CreateWindowExW(
				ex_style,
				class_name.as_ptr(),
				null(),
				style,
				0,
				0,
				10,
				10,
				isize::from(win32_window.hwnd) as winapi::shared::windef::HWND,
				null_mut(),
				null_mut(),
				null_mut(),
			)
		};
		if hwnd.is_null() {
			let message = "WinChildWindow::new(): CreateWindowEx failed";
			return Err(Error::msg(message));
		}

		Ok(Self(hwnd))
	}

	pub fn set_visible(&self, is_visible: bool) {
		unsafe {
			ShowWindow(self.0, if is_visible { SW_SHOW } else { SW_HIDE });
		}
	}

	pub fn update(&self, container: &Window) {
		let container_size = container.size();

		// get the HWND's width/height
		let (width, height) = unsafe {
			let mut rect = zeroed();
			GetWindowRect(self.0, &mut rect);
			((rect.right - rect.left), (rect.bottom - rect.top))
		};
		if width <= 0 && height <= 0 {
			return;
		}
		let aspect_ratio = width as f64 / height as f64;
		let (position, size) = fit_in_size(container_size, aspect_ratio);
		event!(
            LOG,
            "ChildWindow::set_size(): container_size={container_size:?} aspect_ratio={aspect_ratio} position={position:?} size={size:?}"
        );

		let flags = SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING;
		let x = position.x;
		let y = position.y;
		let cx = size.width.try_into().unwrap();
		let cy = size.height.try_into().unwrap();
		unsafe {
			SetWindowPos(self.0, 0 as HWND, x, y, cx, cy, flags);

			let is_visible = IsWindowVisible(self.0) != 0;
			if is_visible && (GetFocus() == GetParent(self.0)) {
				SetFocus(self.0);
			}
		}
	}

	pub fn text(&self) -> Option<String> {
		let hwnd = self.0 as usize;
		Some(hwnd.to_string())
	}
}

fn fit_in_size(container_size: PhysicalSize, aspect_ratio: f64) -> (PhysicalPosition, PhysicalSize) {
	let container_aspect_ratio = container_size.width as f64 / container_size.height as f64;
	let new_size = if container_aspect_ratio <= aspect_ratio {
		PhysicalSize {
			width: container_size.width,
			height: (container_size.width as f64 / aspect_ratio) as u32,
		}
	} else {
		PhysicalSize {
			width: (container_size.height as f64 * aspect_ratio) as u32,
			height: container_size.height,
		}
	};
	let new_position = PhysicalPosition {
		x: i32::try_from((container_size.width as i64 - new_size.width as i64) / 2).unwrap(),
		y: i32::try_from((container_size.height as i64 - new_size.height as i64) / 2).unwrap(),
	};
	(new_position, new_size)
}

#[cfg(test)]
mod test {
	use slint::{PhysicalPosition, PhysicalSize};
	use test_case::test_case;

	#[allow(clippy::too_many_arguments)]
	#[test_case(0, 100, 100, 0.5, 25, 0, 50, 100)]
	#[test_case(1, 100, 100, 1.0, 0, 0, 100, 100)]
	#[test_case(2, 100, 100, 2.0, 0, 25, 100, 50)]
	#[test_case(3, 200, 100, 0.5, 75, 0, 50, 100)]
	#[test_case(4, 200, 100, 1.0, 50, 0, 100, 100)]
	#[test_case(5, 200, 100, 2.0, 0, 0, 200, 100)]
	fn fit_in_size(
		_index: usize,
		container_size_width: u32,
		container_size_height: u32,
		aspect_ratio: f64,
		expected_x: i32,
		expected_y: i32,
		expected_width: u32,
		expected_height: u32,
	) {
		let expected_position = PhysicalPosition {
			x: expected_x,
			y: expected_y,
		};
		let expected_size = PhysicalSize {
			width: expected_width,
			height: expected_height,
		};

		let container_size = PhysicalSize {
			width: container_size_width,
			height: container_size_height,
		};
		let (actual_position, actual_size) = super::fit_in_size(container_size, aspect_ratio);
		let expected = (expected_position, expected_size);
		let actual = (actual_position, actual_size);
		assert_eq!(expected, actual);
	}
}
