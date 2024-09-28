use std::any::Any;
use std::cell::Cell;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use muda::Menu;
use slint::spawn_local;
use slint::Model;
use slint::ModelNotify;
use slint::ModelTracker;
use slint::Weak;

use crate::appcommand::AppCommand;
use crate::guiutils::menuing::MenuDesc;
use crate::info::InfoDb;
use crate::prefs::PrefsCollection;
use crate::ui::AppWindow;
use crate::ui::MagicListViewItem;

pub struct CollectionsViewModel {
	app_window_weak: Weak<AppWindow>,
	info_db: RefCell<Option<Rc<InfoDb>>>,
	items: RefCell<Vec<Rc<PrefsCollection>>>,
	after_refresh_callback: Cell<Option<Box<dyn Future<Output = ()> + 'static>>>,
	notify: ModelNotify,
}

impl CollectionsViewModel {
	pub fn new(app_window_weak: Weak<AppWindow>) -> Self {
		Self {
			app_window_weak,
			info_db: RefCell::new(None),
			items: RefCell::new(Vec::new()),
			after_refresh_callback: Cell::new(None),
			notify: ModelNotify::default(),
		}
	}

	pub fn update(&self, info_db: Option<Rc<InfoDb>>, items: &[Rc<PrefsCollection>]) {
		self.info_db.replace(info_db);
		self.items.replace(items.to_vec());
		self.notify.reset();
	}

	pub fn get_all(&self) -> Vec<Rc<PrefsCollection>> {
		let items = self.items.borrow();
		items.clone()
	}

	pub fn get(&self, index: usize) -> Option<Rc<PrefsCollection>> {
		let items = self.items.borrow();
		items.get(index).cloned()
	}

	pub fn callback_after_refresh(&self, callback: impl Future<Output = ()> + 'static) {
		let callback = Box::new(callback) as Box<dyn Future<Output = ()> + 'static>;
		self.after_refresh_callback.set(Some(callback));
	}

	pub fn context_commands(&self, index: Option<usize>) -> Option<Menu> {
		let mut menu_items = Vec::new();

		// menu items pertaining to selected collections
		if let Some(old_index) = index {
			let items = self.items.borrow();
			if old_index > 0 {
				let new_index = Some(old_index - 1);
				let command = AppCommand::MoveCollection { old_index, new_index };
				menu_items.push(MenuDesc::Item("Move Up".into(), Some(command.into())));
			}
			if old_index < items.len() - 1 {
				let new_index = Some(old_index + 1);
				let command = AppCommand::MoveCollection { old_index, new_index };
				menu_items.push(MenuDesc::Item("Move Down".into(), Some(command.into())));
			}
			if items.len() > 1 {
				let command = AppCommand::DeleteCollectionDialog { index: old_index };
				menu_items.push(MenuDesc::Item("Delete".into(), Some(command.into())));
			}
			if items
				.get(old_index)
				.map(|x| matches!(x.as_ref(), PrefsCollection::Folder { .. }))
				.unwrap_or_default()
			{
				let command = AppCommand::RenameCollectionDialog { index: old_index };
				menu_items.push(MenuDesc::Item("Rename...".into(), Some(command.into())));
			}
			menu_items.push(MenuDesc::Separator);
		}

		// new collection
		let command = AppCommand::AddToNewFolderDialog([].into());
		menu_items.push(MenuDesc::Item("New Collection".into(), Some(command.into())));

		// make the popup menu
		Some(MenuDesc::make_popup_menu(menu_items))
	}
}

impl Model for CollectionsViewModel {
	type Data = MagicListViewItem;

	fn row_count(&self) -> usize {
		invoke_after_refresh_callback(&self.after_refresh_callback);
		if self.info_db.borrow().is_some() {
			self.items.borrow().len()
		} else {
			0
		}
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let info_db = self.info_db.borrow();
		let info_db = info_db.as_ref()?.as_ref();
		self.get(row).map(|item| {
			let prefix_icon = item.icon().slint_icon(&self.app_window_weak.unwrap());
			let text = item.description(info_db).as_ref().into();
			MagicListViewItem {
				prefix_icon,
				text,
				supporting_text: Default::default(),
			}
		})
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

fn invoke_after_refresh_callback(after_refresh_callback: &Cell<Option<Box<dyn Future<Output = ()> + 'static>>>) {
	if let Some(callback) = after_refresh_callback.take() {
		let callback = Pin::from(callback);
		spawn_local(callback).unwrap();
	}
}
