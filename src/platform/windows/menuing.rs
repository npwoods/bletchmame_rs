use anyhow::Result;
use dpi::Position;
use i_slint_backend_winit::WinitWindowAccessor;
use muda::ContextMenu;
use muda::Menu;
use slint::LogicalPosition;
use slint::Window;

use super::get_win32_window_handle;

pub fn attach_menu_bar(window: &Window, menu_bar: &Menu) -> Result<()> {
	let win32_window = get_win32_window_handle(window)?;
	unsafe {
		menu_bar.init_for_hwnd(win32_window.hwnd.into())?;
	}
	Ok(())
}

pub fn show_popup_menu(window: &Window, popup_menu: &Menu, position: LogicalPosition) {
	// get the Win32 window handle
	let win32_window = get_win32_window_handle(window).unwrap();

	// convert the position
	let position = dpi::LogicalPosition {
		x: position.x as f64,
		y: position.y as f64,
	};
	let position = Some(Position::Logical(position));

	// use muda to show the popup menu
	unsafe {
		popup_menu.show_context_menu_for_hwnd(win32_window.hwnd.into(), position);
	}

	// gross hack; see https://github.com/slint-ui/slint/issues/5863 for details
	window.with_winit_window(winit::window::Window::request_redraw);
}
