//! `guiutils` is a module that attempts to enc[r]apsulate logic to fill gaps that would ideally be folded into Slint
//!
//! actual platform specific logic should be in `platform`
mod hook;
pub mod menuing;
pub mod modal;

use i_slint_backend_winit::Backend;
use i_slint_core::items::PointerEvent;
use i_slint_core::items::PointerEventKind;
use slint::platform::PointerEventButton;
use winit::window::WindowAttributes;

use crate::guiutils::hook::create_window_attributes_hook;
use crate::platform::WindowAttributesExt;

fn global_hook(attrs: WindowAttributes) -> WindowAttributes {
	attrs.with_bletchmame_icon()
}

pub fn init_gui_utils() {
	let mut backend = Backend::new().unwrap();
	backend.window_attributes_hook = create_window_attributes_hook(global_hook);
	slint::platform::set_platform(Box::new(backend)).unwrap();
}

pub fn is_context_menu_event(evt: &PointerEvent) -> bool {
	evt.button == PointerEventButton::Right && evt.kind == PointerEventKind::Down
}
