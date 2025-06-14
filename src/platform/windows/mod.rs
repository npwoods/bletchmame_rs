use std::any::Any;
use std::os::windows::process::CommandExt;
use std::process::Command;

use anyhow::Error;
use anyhow::Result;
use easy_ext::ext;
use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use raw_window_handle::Win32WindowHandle;
use slint::Window;
use tracing::info;
use win32job::Job;
use windows::Win32::System::Console::ATTACH_PARENT_PROCESS;
use windows::Win32::System::Console::AttachConsole;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::platform::windows::WindowExtWindows;
use winit::window::WindowAttributes;

pub fn win_platform_init() -> Result<impl Any, Error> {
	// attach to the parent's console - debugging is hell if we don't do this
	unsafe {
		let _ = AttachConsole(ATTACH_PARENT_PROCESS);
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

#[ext(WinCommandExt)]
pub impl Command {
	fn create_no_window(&mut self, flag: bool) -> &mut Self {
		if flag {
			self.creation_flags(CREATE_NO_WINDOW.0);
		};
		self
	}
}

#[ext(WinWindowAttributesExt)]
pub impl WindowAttributes {
	fn with_owner_window(self, owner: &Window) -> Self {
		let win32_window = get_win32_window_handle(owner).unwrap();
		WindowAttributesExtWindows::with_owner_window(self, win32_window.hwnd.into())
	}
}

#[ext(WinWindowExt)]
pub impl Window {
	fn with_muda_menu<T>(&self, callback: impl FnOnce(&::muda::Menu) -> T) -> Option<T> {
		i_slint_backend_winit::WinitWindowAccessor::with_muda_menu(self, callback)
	}

	fn set_enabled_for_modal(&self, enabled: bool) {
		self.with_winit_window(|window| {
			info!(window.id=?window.id(), window.title=?window.title(), enabled=?enabled, "Window::set_enabled_for_modal");
			window.set_enable(enabled);
		});
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
