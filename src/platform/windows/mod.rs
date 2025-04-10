#![allow(dead_code)]
pub mod menuing;

use std::any::Any;
use std::os::windows::process::CommandExt;
use std::process::Command;

use anyhow::Error;
use anyhow::Result;
use easy_ext::ext;
use i_slint_backend_winit::WinitWindowAccessor;
use muda::Menu;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use raw_window_handle::Win32WindowHandle;
use slint::LogicalPosition;
use slint::Window;
use win32job::Job;
use windows::Win32::System::Console::ATTACH_PARENT_PROCESS;
use windows::Win32::System::Console::AttachConsole;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
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

pub trait WinCommandExt {
	fn create_no_window(&mut self, flag: bool) -> &mut Self;
}

impl WinCommandExt for Command {
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
	fn attach_menu_bar(&self, menu_bar: &Menu) -> Result<()> {
		menuing::attach_menu_bar(self, menu_bar)
	}

	fn show_popup_menu(&self, popup_menu: &Menu, position: LogicalPosition) {
		menuing::show_popup_menu(self, popup_menu, position)
	}

	fn set_enabled_for_modal(&self, enabled: bool) {
		self.with_winit_window(|window| window.set_enable(enabled));
	}

	fn ensure_child_focus(&self, child: &winit::window::Window) {
		// hackish method that ensures so-called "appropriate focus"; this really needs
		// to be generalized

		// note that we avoid `Window::focus_window()`, as `winit` has a nasty hack that blasts
		// keystrokes into the window
		if child.is_visible().unwrap_or_default() {
			let do_set_focus = get_win32_window_handle(self)
				.ok()
				.map(|x| unsafe { GetFocus() } == isize::from(x.hwnd) as HWND)
				.unwrap_or_default();

			if do_set_focus {
				if let RawWindowHandle::Win32(child_hwnd) = child.window_handle().unwrap().as_raw() {
					unsafe {
						SetFocus(isize::from(child_hwnd.hwnd) as HWND);
					}
				}
			}
		}
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
