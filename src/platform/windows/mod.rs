pub mod childwnd;
pub mod menuing;

use std::any::Any;
use std::os::windows::process::CommandExt;
use std::process::Command;

use anyhow::Error;
use anyhow::Result;
use i_slint_backend_winit::WinitWindowAccessor;
use muda::Menu;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use raw_window_handle::Win32WindowHandle;
use slint::LogicalPosition;
use slint::Window;
use win32job::Job;
use winapi::um::winbase::CREATE_NO_WINDOW;
use winapi::um::wincon::AttachConsole;
use winapi::um::wincon::ATTACH_PARENT_PROCESS;
use winit::platform::windows::IconExtWindows;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::platform::windows::WindowExtWindows;
use winit::window::Icon;
use winit::window::WindowAttributes;

pub fn win_platform_init() -> Result<impl Any, Error> {
	// attach to the parent's console - debugging is hell if we don't do this
	unsafe {
		AttachConsole(ATTACH_PARENT_PROCESS);
	}

	// we spawn MAME a lot - we want to create a Win32 job so that stray
	// MAMEs never float around
	let job = Job::create()?;
	let mut info = job.query_extended_limit_info()?;
	info.limit_kill_on_job_close();
	job.set_extended_limit_info(&info)?;
	job.assign_current_process()?;

	// and return!
	Ok(job)
}

pub trait WinCommandExt {
	fn create_no_window(&mut self, flag: bool) -> &mut Self;
}

impl WinCommandExt for Command {
	fn create_no_window(&mut self, flag: bool) -> &mut Self {
		if flag {
			self.creation_flags(CREATE_NO_WINDOW);
		};
		self
	}
}

fn bletchmame_icon() -> Icon {
	Icon::from_resource(32512, None).unwrap()
}

pub trait WinWindowAttributesExt {
	fn with_bletchmame_icon(self) -> Self;
	fn with_owner_window(self, owner: &Window) -> Self;
}

impl WinWindowAttributesExt for WindowAttributes {
	fn with_bletchmame_icon(self) -> Self {
		let icon = bletchmame_icon();
		self.with_window_icon(Some(icon.clone())).with_taskbar_icon(Some(icon))
	}

	fn with_owner_window(self, owner: &Window) -> Self {
		let win32_window = get_win32_window_handle(owner).unwrap();
		WindowAttributesExtWindows::with_owner_window(self, win32_window.hwnd.into())
	}
}

pub trait WinWindowExt {
	fn attach_menu_bar(&self, menu_bar: &Menu) -> Result<()>;
	fn show_popup_menu(&self, popup_menu: &Menu, point: LogicalPosition);
	fn set_enabled_for_modal(&self, enabled: bool);
}

impl WinWindowExt for Window {
	fn attach_menu_bar(&self, menu_bar: &Menu) -> Result<()> {
		menuing::attach_menu_bar(self, menu_bar)
	}

	fn show_popup_menu(&self, popup_menu: &Menu, point: LogicalPosition) {
		menuing::show_popup_menu(self, popup_menu, point)
	}

	fn set_enabled_for_modal(&self, enabled: bool) {
		self.with_winit_window(|window| window.set_enable(enabled));
	}
}

fn get_win32_window_handle(window: &Window) -> Result<Win32WindowHandle> {
	if let RawWindowHandle::Win32(win32_window) = window.window_handle().window_handle().unwrap().as_raw() {
		Ok(win32_window)
	} else {
		let message = "RawWindowHandle is not RawWindowHandle::Win32";
		Err(Error::msg(message))
	}
}

#[cfg(test)]
mod test {
	#[test]
	fn bletchmame_icon() {
		let _ = super::bletchmame_icon();
	}
}
