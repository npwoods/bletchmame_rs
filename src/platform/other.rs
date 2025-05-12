#![cfg_attr(target_os = "windows", allow(dead_code))]

use std::any::Any;
use std::process::Command;

use anyhow::Result;
use easy_ext::ext;
use slint::Window;
use winit::window::WindowAttributes;

pub fn other_platform_init() -> Result<impl Any> {
	Ok(())
}

#[ext(OtherCommandExt)]
pub impl Command {
	fn create_no_window(&mut self, _flag: bool) -> &mut Self {
		self
	}
}

#[ext(OtherWindowAttributesExt)]
pub impl WindowAttributes {
	fn with_owner_window(self, _owner: &Window) -> Self {
		self
	}
}

#[ext(OtherWindowExt)]
pub impl Window {
	fn set_enabled_for_modal(&self, _enabled: bool) {
		// do nothing for now
	}

	fn with_muda_menu<T>(&self, _callback: impl FnOnce(&::muda::Menu) -> T) -> Option<T> {
		None
	}
}
