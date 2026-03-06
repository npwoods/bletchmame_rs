#![cfg_attr(any(windows, unix), allow(dead_code))]

use std::any::Any;
use std::fs::File;
use std::process::Child;
use std::process::Command;

use anyhow::Error;
use anyhow::Result;
use easy_ext::ext;
use raw_window_handle::RawWindowHandle;
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

	#[allow(dead_code)]
	fn create_new_console(&mut self) -> &mut Self {
		self
	}
}

#[ext(OtherWindowAttributesExt)]
pub impl WindowAttributes {
	fn with_owner_window_handle(self, _owner: &RawWindowHandle) -> Self {
		self
	}
}

#[ext(OtherWindowExt)]
pub impl Window {
	fn set_enabled_for_modal(&self, _enabled: bool) {
		// do nothing for now
	}
}

pub fn other_console_init(_title: &str) -> Result<(Child, File)> {
	Err(Error::msg("Console is not supported on this platform"))
}
