#![cfg_attr(target_os = "windows", allow(dead_code))]

use std::any::Any;
use std::process::Command;

use anyhow::Result;
use muda::Menu;
use slint::LogicalPosition;
use slint::Window;
use winit::window::WindowAttributes;

pub fn other_platform_init() -> Result<impl Any> {
	Ok(())
}

pub trait OtherCommandExt {
	fn create_no_window(&mut self, flag: bool) -> &mut Self;
}

impl OtherCommandExt for Command {
	fn create_no_window(&mut self, _flag: bool) -> &mut Self {
		self
	}
}

pub trait OtherWindowAttributesExt {
	fn with_owner_window(self, owner: &Window) -> Self;
}

impl OtherWindowAttributesExt for WindowAttributes {
	fn with_owner_window(self, _owner: &Window) -> Self {
		self
	}
}

pub trait OtherWindowExt {
	fn attach_menu_bar(&self, menu_bar: &Menu) -> Result<()>;
	fn show_popup_menu(&self, popup_menu: &Menu, point: LogicalPosition);
	fn set_enabled_for_modal(&self, enabled: bool);
	fn ensure_child_focus(&self, child: &winit::window::Window);
}

impl OtherWindowExt for Window {
	fn attach_menu_bar(&self, _menu_bar: &Menu) -> Result<()> {
		todo!()
	}

	fn show_popup_menu(&self, _popup_menu: &Menu, _position: LogicalPosition) {
		todo!()
	}

	fn set_enabled_for_modal(&self, _enabled: bool) {
		// do nothing for now
	}

	fn ensure_child_focus(&self, _child: &winit::window::Window) {
		// do nothing for now
	}
}
