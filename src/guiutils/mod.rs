//! `guiutils` is a module that attempts to enc[r]apsulate logic to fill gaps that would ideally be folded into Slint
//!
//! actual platform specific logic should be in `platform`
mod component;
mod hook;
pub mod menuing;
pub mod modal;

use anyhow::Result;
use i_slint_core::items::PointerEvent;
use i_slint_core::items::PointerEventKind;
use slint::platform::PointerEventButton;
use strum::EnumString;
use winit::window::WindowAttributes;

use crate::guiutils::hook::create_window_attributes_hook;

fn global_hook(attrs: WindowAttributes) -> WindowAttributes {
	attrs
}

#[derive(Debug, EnumString)]
pub enum SlintBackend {
	#[strum(ascii_case_insensitive)]
	Winit,

	#[cfg(feature = "slint-qt-backend")]
	#[strum(ascii_case_insensitive)]
	Qt,
}

impl Default for SlintBackend {
	fn default() -> Self {
		#[cfg(feature = "slint-qt-backend")]
		let result = Self::Qt;

		#[cfg(not(feature = "slint-qt-backend"))]
		let result = Self::Winit;

		result
	}
}

pub fn init_slint_backend(backend_type: SlintBackend) -> Result<()> {
	// create the backend
	let backend = match backend_type {
		SlintBackend::Winit => {
			let mut backend = i_slint_backend_winit::Backend::new()?;
			backend.window_attributes_hook = create_window_attributes_hook(global_hook);
			Box::new(backend) as Box<_>
		}

		#[cfg(feature = "slint-qt-backend")]
		SlintBackend::Qt => Box::new(i_slint_backend_qt::Backend::new()) as Box<_>,
	};

	// and set it up
	slint::platform::set_platform(backend)?;
	Ok(())
}

pub fn is_context_menu_event(evt: &PointerEvent) -> bool {
	evt.button == PointerEventButton::Right && evt.kind == PointerEventKind::Down
}
