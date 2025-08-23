//! Helpers for Menu handling; which Slint does not handle yet
use std::borrow::Cow;
use std::convert::Infallible;
use std::ops::ControlFlow;

use easy_ext::ext;
use muda::Menu;
use muda::MenuItemKind;
use muda::Submenu;
use muda::accelerator::Accelerator;
use muda::accelerator::Code;
use muda::accelerator::Modifiers;

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
		"F8" => Code::F8,
		"F9" => Code::F9,
		"F10" => Code::F10,
		"F11" => Code::F11,
		"F12" => Code::F12,
		"Pause" => Code::Pause,
		"ScrLk" => Code::ScrollLock,
		x => panic!("Unknown accelerator {x}"),
	};
	Some(Accelerator::new(mods, key))
}

#[derive(Debug, Default)]
pub struct MenuItemUpdate {
	pub text: Option<Cow<'static, str>>,
}

/// Extension for muda menus
#[ext(MenuExt)]
pub impl Menu {
	fn update(&self, callback: impl Fn(Option<&str>, &str) -> MenuItemUpdate) {
		let _ = self.visit((), |_, sub_menu, item| {
			if let Some(title) = item.text() {
				let parent_title = sub_menu.map(|x| x.text());
				let update = callback(parent_title.as_deref(), &title);
				if let Some(text) = update.text {
					item.set_text(&text);
				}
			}
			ControlFlow::<Infallible>::Continue(())
		});
	}

	fn visit<B, C>(
		&self,
		init: C,
		func: impl Fn(C, Option<&Submenu>, &MenuItemKind) -> ControlFlow<B, C>,
	) -> ControlFlow<B, C> {
		visit_menu_items(&self.items(), init, None, &func)
	}
}

#[ext(MenuItemKindExt)]
pub impl MenuItemKind {
	fn text(&self) -> Option<String> {
		match self {
			MenuItemKind::MenuItem(menu_item) => Some(menu_item.text()),
			MenuItemKind::Check(check_menu_item) => Some(check_menu_item.text()),
			_ => None,
		}
	}

	fn set_text(&self, text: impl AsRef<str>) {
		match self {
			MenuItemKind::MenuItem(menu_item) => menu_item.set_text(text),
			MenuItemKind::Check(check_menu_item) => check_menu_item.set_text(text),
			_ => todo!(),
		}
	}
}

fn visit_menu_items<B, C>(
	items: &[MenuItemKind],
	init: C,
	sub_menu: Option<&Submenu>,
	func: &impl Fn(C, Option<&Submenu>, &MenuItemKind) -> ControlFlow<B, C>,
) -> ControlFlow<B, C> {
	items
		.iter()
		.try_fold(init, |state, item| match func(state, sub_menu, item) {
			ControlFlow::Break(x) => ControlFlow::Break(x),
			ControlFlow::Continue(x) => {
				if let MenuItemKind::Submenu(sub_menu) = item {
					visit_menu_items(&sub_menu.items(), x, Some(sub_menu), func)
				} else {
					ControlFlow::Continue(x)
				}
			}
		})
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
