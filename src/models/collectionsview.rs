use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use slint::Model;
use slint::ModelNotify;
use slint::ModelTracker;
use slint::Weak;

use crate::action::Action;
use crate::info::InfoDb;
use crate::prefs::PrefsCollection;
use crate::ui::AppWindow;
use crate::ui::CollectionContextMenuInfo;
use crate::ui::NavigationItem;

pub struct CollectionsViewModel {
	app_window_weak: Weak<AppWindow>,
	info_db: RefCell<Option<Rc<InfoDb>>>,
	items: RefCell<Vec<Rc<PrefsCollection>>>,
	notify: ModelNotify,
}

impl CollectionsViewModel {
	pub fn new(app_window_weak: Weak<AppWindow>) -> Self {
		Self {
			app_window_weak,
			info_db: RefCell::new(None),
			items: RefCell::new(Vec::new()),
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

	pub fn context_commands(&self, index: Option<usize>) -> Option<CollectionContextMenuInfo> {
		let mut result = CollectionContextMenuInfo::default();

		// menu items pertaining to selected collections
		if let Some(old_index) = index {
			let items = self.items.borrow();
			if old_index > 0 {
				let new_index = Some(old_index - 1);
				let command = Action::MoveCollection { old_index, new_index };
				result.move_up_command = command.encode_for_slint();
			}
			if old_index < items.len() - 1 {
				let new_index = Some(old_index + 1);
				let command = Action::MoveCollection { old_index, new_index };
				result.move_down_command = command.encode_for_slint();
			}
			if items.len() > 1 {
				let command = Action::DeleteCollectionDialog { index: old_index };
				result.delete_command = command.encode_for_slint();
			}
			if items
				.get(old_index)
				.map(|x| matches!(x.as_ref(), PrefsCollection::Folder { .. }))
				.unwrap_or_default()
			{
				let command = Action::RenameCollectionDialog { index: old_index };
				result.rename_command = command.encode_for_slint();
			}
		}

		// new collection
		let command = Action::AddToNewFolderDialog([].into());
		result.new_collection_command = command.encode_for_slint();

		// make the popup menu
		Some(result)
	}
}

impl Model for CollectionsViewModel {
	type Data = NavigationItem;

	fn row_count(&self) -> usize {
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
			let icon = item.icon().slint_icon(&self.app_window_weak.unwrap());
			let text = item.description(info_db).as_ref().into();
			NavigationItem {
				icon,
				text,
				..Default::default()
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
