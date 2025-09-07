//! `guiutils` is a module that attempts to enc[r]apsulate logic to fill gaps that would ideally be folded into Slint
//!
//! actual platform specific logic should be in `platform`
pub mod modal;

use i_slint_core::items::PointerEvent;
use i_slint_core::items::PointerEventKind;
use slint::platform::PointerEventButton;

pub fn is_context_menu_event(evt: &PointerEvent) -> bool {
	evt.button == PointerEventButton::Right && evt.kind == PointerEventKind::Down
}
