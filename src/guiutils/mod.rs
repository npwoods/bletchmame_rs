//! `guiutils` is a module that attempts to enc[r]apsulate various platform-specific GUI aspects that would ideally be folded into Slint
pub mod childwnd;
pub mod menuing;
pub mod windowing;

use std::cell::RefCell;

use i_slint_backend_winit::Backend;
use i_slint_core::items::PointerEvent;
use i_slint_core::items::PointerEventKind;
use slint::platform::PointerEventButton;
use winit::window::WindowAttributes;

type WindowAttributeHookCallback = Box<dyn Fn(WindowAttributes) -> WindowAttributes + 'static>;
thread_local! {
	static WINDOW_ATTRIBUTE_HOOK_CALLBACK: RefCell<Option<WindowAttributeHookCallback>> = const { RefCell::new(None) }
}

fn window_attributes_hook(attrs: WindowAttributes) -> WindowAttributes {
	WINDOW_ATTRIBUTE_HOOK_CALLBACK.with_borrow(|callback| {
		if let Some(callback) = callback {
			callback(attrs)
		} else {
			attrs
		}
	})
}

pub fn init_gui_utils() {
	let mut backend = Backend::new().unwrap();
	backend.window_attributes_hook = Some(Box::new(window_attributes_hook));
	slint::platform::set_platform(Box::new(backend)).unwrap();
}

pub fn is_context_menu_event(evt: &PointerEvent) -> bool {
	evt.button == PointerEventButton::Right && evt.kind == PointerEventKind::Down
}
