//! Helpers for Menu handling; which Slint does not handle yet
use std::convert::Infallible;
use std::ops::ControlFlow;

use i_slint_core::items::MenuEntry as SlintMenuEntry;
use muda::IsMenuItem;
use muda::Menu;
use muda::MenuId;
use muda::MenuItem;
use muda::MenuItemKind;
use muda::PredefinedMenuItem;
use muda::Submenu;
use muda::accelerator::Accelerator;
use muda::accelerator::Code;
use muda::accelerator::Modifiers;
use slint::ModelRc;
use slint::VecModel;

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
		"Pause" => Code::Pause,
		x => panic!("Unknown accelerator {x}"),
	};
	Some(Accelerator::new(mods, key))
}

#[derive(Debug, Default)]
pub struct MenuItemUpdate {
	pub enabled: Option<bool>,
	pub checked: Option<bool>,
}

/// Extension for muda menus
pub trait MenuExt {
	fn update(&self, callback: impl Fn(&MenuId) -> MenuItemUpdate);
	fn slint_menu_entries(&self, sub_menu: Option<&SlintMenuEntry>) -> ModelRc<SlintMenuEntry>;
	fn is_natively_supported() -> bool;
	fn visit<B, C>(&self, init: C, func: impl Fn(C, &MenuItemKind) -> ControlFlow<B, C>) -> ControlFlow<B, C>;
}

impl MenuExt for Menu {
	fn update(&self, callback: impl Fn(&MenuId) -> MenuItemUpdate) {
		self.visit((), |_, item| {
			match item {
				MenuItemKind::MenuItem(menu_item) => {
					let update = callback(menu_item.id());
					if let Some(enabled) = update.enabled {
						menu_item.set_enabled(enabled);
					}
					assert!(
						update.checked.is_none(),
						"Menu item \"{}\" needs to be using CheckMenuItem",
						menu_item.text()
					);
				}
				MenuItemKind::Check(menu_item) => {
					let update = callback(menu_item.id());
					if let Some(enabled) = update.enabled {
						menu_item.set_enabled(enabled);
					}
					if let Some(checked) = update.checked {
						menu_item.set_checked(checked);
					}
				}
				_ => {
					// do nothing
				}
			};
			ControlFlow::<Infallible>::Continue(())
		});
	}

	fn slint_menu_entries(&self, sub_menu: Option<&SlintMenuEntry>) -> ModelRc<SlintMenuEntry> {
		// find the menu items we want to return
		let items = if let Some(sub_menu) = sub_menu {
			let title = &sub_menu.title;
			let flow = self.visit((), |_, item: &MenuItemKind| {
				if let Some(items) = item
					.as_submenu()
					.and_then(|sub_menu| (sub_menu.text().as_str() == title.as_str()).then(|| sub_menu.items()))
				{
					ControlFlow::Break(items)
				} else {
					ControlFlow::Continue(())
				}
			});
			match flow {
				ControlFlow::Break(items) => items,
				ControlFlow::Continue(()) => Vec::new(),
			}
		} else {
			self.items()
		};

		// convert them to Slint
		let items = items.iter().filter_map(slint_menu_entry).collect::<Vec<_>>();

		// and build the model
		let model = VecModel::from(items);
		ModelRc::new(model)
	}

	fn is_natively_supported() -> bool {
		cfg!(windows)
	}

	fn visit<B, C>(&self, init: C, func: impl Fn(C, &MenuItemKind) -> ControlFlow<B, C>) -> ControlFlow<B, C> {
		visit_menu_items(&self.items(), init, &func)
	}
}

fn visit_menu_items<B, C>(
	items: &[MenuItemKind],
	init: C,
	func: &impl Fn(C, &MenuItemKind) -> ControlFlow<B, C>,
) -> ControlFlow<B, C> {
	items.iter().try_fold(init, |state, item| match func(state, item) {
		ControlFlow::Break(x) => ControlFlow::Break(x),
		ControlFlow::Continue(x) => {
			if let MenuItemKind::Submenu(sub_menu) = item {
				visit_menu_items(&sub_menu.items(), x, func)
			} else {
				ControlFlow::Continue(x)
			}
		}
	})
}

fn slint_menu_entry(menu_item: &MenuItemKind) -> Option<SlintMenuEntry> {
	let (title, id, has_sub_menu) = match menu_item {
		MenuItemKind::MenuItem(menu_item) => Some((menu_item.text(), menu_item.id().as_ref(), false)),
		MenuItemKind::Check(menu_item) => Some((menu_item.text(), menu_item.id().as_ref(), false)),
		MenuItemKind::Submenu(menu_item) => Some((menu_item.text(), menu_item.id().as_ref(), true)),
		_ => None,
	}?;

	let title = title.into();
	let id = id.into();
	let entry = SlintMenuEntry {
		title,
		id,
		has_sub_menu,
	};
	Some(entry)
}

#[derive(Debug)]
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
