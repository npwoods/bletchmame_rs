//! `guiutils` is a module that attempts to enc[r]apsulate various platform-specific GUI aspects that would ideally be folded into Slint
pub mod menuing;
pub mod windowing;

use std::cell::Cell;

use i_slint_backend_winit::Backend;
use i_slint_core::items::PointerEvent;
use i_slint_core::items::PointerEventKind;
use raw_window_handle::Win32WindowHandle;
use slint::platform::PointerEventButton;
use winit::platform::windows::WindowAttributesExtWindows;

thread_local! {
	static MODAL_PARENT: Cell<Option<Win32WindowHandle>> = const { Cell::new(None) }
}

fn window_builder_hook(window_builder: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
	if let Some(modal_parent) = MODAL_PARENT.get() {
		window_builder.with_owner_window(modal_parent.hwnd.into())
	} else {
		window_builder
	}
}

pub fn init_gui_utils() {
	let mut backend = Backend::new().unwrap();
	backend.window_builder_hook = Some(Box::new(window_builder_hook));
	slint::platform::set_platform(Box::new(backend)).unwrap();
}

pub fn is_context_menu_event(evt: &PointerEvent) -> bool {
	evt.button == PointerEventButton::Right && evt.kind == PointerEventKind::Down
}
