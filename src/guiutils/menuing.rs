//! Helpers for Menu handling; which Sling does not handle yet
use std::mem::zeroed;

use itertools::Either;
use muda::accelerator::Accelerator;
use muda::accelerator::Code;
use muda::accelerator::Modifiers;
use muda::ContextMenu;
use muda::IsMenuItem;
use muda::Menu;
use muda::MenuId;
use muda::MenuItem;
use muda::MenuItemKind;
use muda::PredefinedMenuItem;
use muda::Submenu;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::LogicalPosition;
use slint::Window;
use winapi::shared::windef::HWND;
use winapi::um::winuser::GetWindowRect;
use winapi::um::winuser::SetWindowPos;
use winapi::um::winuser::SWP_NOACTIVATE;
use winapi::um::winuser::SWP_NOMOVE;
use winapi::um::winuser::SWP_NOOWNERZORDER;
use winapi::um::winuser::SWP_NOSENDCHANGING;

/// Helper function to declare accelerators
pub fn accel(text: &str) -> Option<Accelerator> {
	fn strip_modifier<'a>(
		text: &'a str,
		mods: Option<Modifiers>,
		prefix: &str,
		prefix_mod: Modifiers,
	) -> (&'a str, Option<Modifiers>) {
		text.strip_prefix(prefix)
			.map(|text| (text, Some(mods.unwrap_or_default() | prefix_mod)))
			.unwrap_or((text, mods))
	}

	let mods = None;
	let (text, mods) = strip_modifier(text, mods, "Ctrl+", Modifiers::CONTROL);
	let (text, mods) = strip_modifier(text, mods, "Shift+", Modifiers::SHIFT);
	let (text, mods) = strip_modifier(text, mods, "Alt+", Modifiers::ALT);

	let key = match text {
		"X" => Code::KeyX,
		"F7" => Code::F7,
		"Pause" => Code::Pause,
		_ => panic!("Unknown accelerator"),
	};
	Some(Accelerator::new(mods, key))
}

pub fn setup_window_menu_bar(window: &Window, menu_bar: &Menu) {
	let raw_window = window.window_handle().window_handle().unwrap().as_raw();
	match raw_window {
		#[cfg(target_os = "windows")]
		RawWindowHandle::Win32(win32_window) => {
			menu_bar.init_for_hwnd(win32_window.hwnd.into()).unwrap();
		}
		_ => panic!("Unknown RawWindowHandle type"),
	}
}

pub fn iterate_menu_items(menu: &Menu) -> impl Iterator<Item = MenuItem> {
	iterate_menu_items_internal(menu.items())
}

fn iterate_menu_items_internal(items: Vec<MenuItemKind>) -> impl Iterator<Item = MenuItem> {
	items.into_iter().flat_map(|item| match item {
		MenuItemKind::MenuItem(menu_item) => Either::Left([menu_item].into_iter()),
		MenuItemKind::Submenu(sub_menu) => Either::Right(
			iterate_menu_items_internal(sub_menu.items())
				.collect::<Vec<_>>()
				.into_iter(),
		),
		_ => Either::Right(Vec::new().into_iter()),
	})
}

pub enum MenuDesc {
	Item(String, Option<MenuId>),
	SubMenu(String, bool, Vec<MenuDesc>),
	Separator,
}

impl MenuDesc {
	pub fn into_boxed_menu_item(self) -> Box<dyn IsMenuItem + 'static> {
		match self {
			MenuDesc::Item(text, id) => {
				let menu_item = if let Some(id) = id {
					MenuItem::with_id(id, text, true, None)
				} else {
					MenuItem::new(text, false, None)
				};
				Box::new(menu_item)
			}
			MenuDesc::SubMenu(text, enabled, items) => {
				let items = items.into_iter().map(|x| x.into_boxed_menu_item()).collect::<Vec<_>>();
				let items = items.iter().map(|x| &**x as &dyn IsMenuItem).collect::<Vec<_>>();
				let submenu = Submenu::with_items(&text, enabled, &items).unwrap();
				Box::new(submenu)
			}
			MenuDesc::Separator => {
				let menu_item = PredefinedMenuItem::separator();
				Box::new(menu_item)
			}
		}
	}

	pub fn make_popup_menu(items: impl IntoIterator<Item = Self>) -> Menu {
		let items = items.into_iter().map(|x| x.into_boxed_menu_item()).collect::<Vec<_>>();
		let items = items.iter().map(|x| &**x as &dyn IsMenuItem).collect::<Vec<_>>();
		Menu::with_items(&items).unwrap()
	}
}

pub fn show_popup_menu(window: &Window, popup_menu: &Menu, _point: LogicalPosition) {
	let raw_window = window.window_handle().window_handle().unwrap().as_raw();
	match raw_window {
		#[cfg(target_os = "windows")]
		RawWindowHandle::Win32(win32_window) => {
			// use tauri to show the popup menu
			popup_menu.show_context_menu_for_hwnd(win32_window.hwnd.into(), None);

			// very gross hack
			unfreeze_slint_after_popup_menu_hack(isize::from(win32_window.hwnd) as HWND);
		}
		_ => panic!("Unknown RawWindowHandle type"),
	}
}

/// gross hack to work around Slint freezes
fn unfreeze_slint_after_popup_menu_hack(hwnd: HWND) {
	// see https://github.com/slint-ui/slint/issues/5863 for details
	unsafe {
		// get the HWND's width/height
		let (width, height) = {
			let mut rect = zeroed();
			GetWindowRect(hwnd, &mut rect);
			(rect.right - rect.left, rect.bottom - rect.top)
		};

		// make the window a single pixel wider, and flip it back - the act of changing the size
		// seems to "tickle" Slint into unfreezing
		let flags = SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING;
		SetWindowPos(hwnd, 0 as HWND, 0, 0, width + 1, height, flags);
		SetWindowPos(hwnd, 0 as HWND, 0, 0, width, height, flags);
	}
}

#[cfg(test)]
mod test {
	use muda::accelerator::Accelerator;
	use muda::accelerator::Code;
	use muda::accelerator::Modifiers;
	use test_case::test_case;

	#[test_case(0, "X", Accelerator::new(None, Code::KeyX))]
	#[test_case(1, "Ctrl+X", Accelerator::new(Some(Modifiers::CONTROL), Code::KeyX))]
	#[test_case(2, "Shift+X", Accelerator::new(Some(Modifiers::SHIFT), Code::KeyX))]
	#[test_case(3, "Ctrl+Alt+X", Accelerator::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyX))]
	pub fn accel(_index: usize, text: &str, expected: Accelerator) {
		let actual = super::accel(text);
		assert_eq!(Some(expected), actual);
	}
}
