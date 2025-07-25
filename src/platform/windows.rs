use std::any::Any;
use std::ffi::c_void;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::RawHandle;
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
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::CreateFileW;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
use windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE;
use windows::Win32::Storage::FileSystem::OPEN_EXISTING;
use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows::Win32::System::Console::ATTACH_PARENT_PROCESS;
use windows::Win32::System::Console::AllocConsole;
use windows::Win32::System::Console::AttachConsole;
use windows::Win32::System::Console::CONSOLE_MODE;
use windows::Win32::System::Console::ENABLE_VIRTUAL_TERMINAL_PROCESSING;
use windows::Win32::System::Console::FreeConsole;
use windows::Win32::System::Console::GetConsoleMode;
use windows::Win32::System::Console::SetConsoleMode;
use windows::Win32::System::Pipes::ConnectNamedPipe;
use windows::Win32::System::Pipes::CreateNamedPipeW;
use windows::Win32::System::Pipes::PIPE_READMODE_MESSAGE;
use windows::Win32::System::Pipes::PIPE_TYPE_MESSAGE;
use windows::Win32::System::Pipes::PIPE_WAIT;
use windows::Win32::System::Threading::CREATE_NEW_CONSOLE;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::UI::WindowsAndMessaging::GetMenu;
use windows::Win32::UI::WindowsAndMessaging::GetPropW;
use windows::Win32::UI::WindowsAndMessaging::HMENU;
use windows::Win32::UI::WindowsAndMessaging::SetMenu;
use windows::Win32::UI::WindowsAndMessaging::SetPropW;
use windows::core::PCWSTR;
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

	fn create_new_console(&mut self) -> &mut Self {
		self.creation_flags(CREATE_NEW_CONSOLE.0);
		self
	}
}

#[ext(WinWindowAttributesExt)]
pub impl WindowAttributes {
	fn with_owner_window_handle(self, owner: &RawWindowHandle) -> Self {
		let RawWindowHandle::Win32(owner) = owner else {
			unreachable!();
		};
		self.with_owner_window(owner.hwnd.into())
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

	fn is_menu_bar_visible(&self) -> Option<bool> {
		// access the HWND
		let win32_window_handle = get_win32_window_handle(self).ok()?;
		let hwnd = HWND(win32_window_handle.hwnd.get() as usize as *mut c_void);

		// do we have a menu?
		let hmenu = unsafe { GetMenu(hwnd) };
		Some(!hmenu.is_invalid())
	}

	fn set_menu_bar_visible(&self, visible: bool) {
		// access the HWND
		let win32_window_handle = get_win32_window_handle(self).unwrap();
		let hwnd = HWND(win32_window_handle.hwnd.get() as usize as *mut c_void);

		// do we have a menu?
		let old_hmenu = unsafe { GetMenu(hwnd) };

		// are we changing anything?
		if old_hmenu.is_invalid() == visible {
			// this is the property name
			let prop_name = "bletchmame_menu_bar\0".encode_utf16().collect::<Vec<_>>();
			let prop_name = PCWSTR(prop_name.as_ptr());

			// toggle the menu state
			let new_hmenu = if visible {
				unsafe { Some(HMENU(GetPropW(hwnd, prop_name).0)) }
			} else {
				unsafe { SetPropW(hwnd, prop_name, Some(HANDLE(old_hmenu.0))).unwrap() };
				None
			};
			unsafe { SetMenu(hwnd, new_hmenu).unwrap() };
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

/// Windows specific "echo console" - this is simpler on other platforms
pub fn win_echo_console_main(pipe_name: &str) -> Result<()> {
	let mut output = unsafe {
		FreeConsole()?;
		AllocConsole()?;

		let output = CreateFileW(
			PCWSTR("CONOUT$\0".encode_utf16().collect::<Vec<u16>>().as_ptr()),
			GENERIC_READ.0 | GENERIC_WRITE.0,
			FILE_SHARE_WRITE | FILE_SHARE_READ,
			None,
			OPEN_EXISTING,
			FILE_ATTRIBUTE_NORMAL,
			None,
		)?;

		// enable ANSI
		let mut mode = CONSOLE_MODE::default();
		GetConsoleMode(output, &mut mode)?;
		SetConsoleMode(output, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING)?;

		// return the file
		File::from_raw_handle(output.0 as _)
	};

	let input = File::open(pipe_name)?;
	let input = BufReader::new(input);
	for line in input.lines() {
		let line = line?;
		writeln!(output, "{line}")?;
	}
	Ok(())
}

#[derive(Debug)]
pub struct WinNamedPipe(HANDLE);

impl WinNamedPipe {
	pub fn new(name: &str) -> Result<Self> {
		let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
		let handle = unsafe {
			CreateNamedPipeW(
				PCWSTR(name_wide.as_ptr()),
				PIPE_ACCESS_DUPLEX,
				PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
				1,    // max instances
				1024, // out buffer size
				1024, // in buffer size
				0,    // default timeout
				None, // default security
			)
		};
		if handle.is_invalid() {
			let message = "CreateNamedPipeW() failed";
			return Err(Error::msg(message));
		}
		Ok(Self(handle))
	}

	pub fn connect(self) -> Result<File> {
		unsafe {
			ConnectNamedPipe(self.0, None)?;
			Ok(File::from_raw_handle(self.0.0 as RawHandle))
		}
	}
}
