//! Helpers for Menu handling; which Sling does not handle yet
use itertools::Either;
use muda::accelerator::Accelerator;
use muda::accelerator::Code;
use muda::accelerator::Modifiers;
use muda::ContextMenu;
use muda::Menu;
use muda::MenuItem;
use muda::MenuItemKind;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use slint::LogicalPosition;
use slint::Window;

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

pub fn show_popup_menu(window: &Window, popup_menu: &Menu, _point: LogicalPosition) {
	let raw_window = window.window_handle().window_handle().unwrap().as_raw();
	match raw_window {
		#[cfg(target_os = "windows")]
		RawWindowHandle::Win32(win32_window) => {
			popup_menu.show_context_menu_for_hwnd(win32_window.hwnd.into(), None);
		}
		_ => panic!("Unknown RawWindowHandle type"),
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
