use std::mem::zeroed;

use anyhow::Result;
use muda::ContextMenu;
use muda::Menu;
use slint::LogicalPosition;
use slint::Window;
use winapi::shared::windef::HWND;
use winapi::um::winuser::GetWindowRect;
use winapi::um::winuser::SetWindowPos;
use winapi::um::winuser::SWP_NOACTIVATE;
use winapi::um::winuser::SWP_NOMOVE;
use winapi::um::winuser::SWP_NOOWNERZORDER;
use winapi::um::winuser::SWP_NOSENDCHANGING;

use super::get_win32_window_handle;

pub fn attach_menu_bar(window: &Window, menu_bar: &Menu) -> Result<()> {
	let win32_window = get_win32_window_handle(window)?;
	unsafe {
		menu_bar.init_for_hwnd(win32_window.hwnd.into())?;
	}
	Ok(())
}

pub fn show_popup_menu(window: &Window, popup_menu: &Menu, _position: LogicalPosition) {
	// get the Win32 window handle
	let win32_window = get_win32_window_handle(window).unwrap();

	// use muda to show the popup menu
	unsafe {
		popup_menu.show_context_menu_for_hwnd(win32_window.hwnd.into(), None);
	}

	// very gross hack
	unfreeze_slint_after_popup_menu_hack(isize::from(win32_window.hwnd) as HWND);
}

/// gross hack to work around Slint freezes
fn unfreeze_slint_after_popup_menu_hack(hwnd: HWND) {
	// see https://github.com/slint-ui/slint/issues/5863 for details
	unsafe {
		// get the HWND's width/height
		let (width, height) = {
			let mut rect = zeroed();
			GetWindowRect(hwnd, &mut rect);
			(rect.right - rect.left, rect.bottom - rect.top)
		};

		// make the window a single pixel wider, and flip it back - the act of changing the size
		// seems to "tickle" Slint into unfreezing
		let flags = SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING;
		SetWindowPos(hwnd, 0 as HWND, 0, 0, width + 1, height, flags);
		SetWindowPos(hwnd, 0 as HWND, 0, 0, width, height, flags);
	}
}
