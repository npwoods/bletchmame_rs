use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use slint::Global;
use slint::Model;
use slint::ModelNotify;
use slint::ModelTracker;
use slint::SharedString;
use slint::Weak;

use crate::prefs::PrefsCollection;
use crate::ui::AppWindow;
use crate::ui::Icons;
use crate::ui::MagicListViewItem;

pub struct CollectionsViewModel {
	app_window_weak: Weak<AppWindow>,
	items: RefCell<Vec<Rc<PrefsCollection>>>,
	notify: ModelNotify,
}

impl CollectionsViewModel {
	pub fn new(app_window_weak: Weak<AppWindow>) -> Self {
		Self {
			app_window_weak,
			items: RefCell::new(Vec::new()),
			notify: ModelNotify::default(),
		}
	}

	pub fn update(&self, items: &[Rc<PrefsCollection>]) {
		self.items.replace(items.iter().cloned().collect());
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
}

impl Model for CollectionsViewModel {
	type Data = MagicListViewItem;

	fn row_count(&self) -> usize {
		self.items.borrow().len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		self.get(row).map(|item| item_display(&self.app_window_weak, &item))
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

fn item_display(app_window_weak: &Weak<AppWindow>, collection: &PrefsCollection) -> MagicListViewItem {
	let app_window = app_window_weak.unwrap();
	let icons = Icons::get(&app_window);
	let (prefix_icon, text) = match collection {
		PrefsCollection::Builtin(x) => (icons.get_search(), format!("{x}").into()),
		PrefsCollection::MachineSoftware { machine_name: _ } => todo!(),
		PrefsCollection::Folder { name, items: _ } => (icons.get_folder(), SharedString::from(name)),
	};
	MagicListViewItem { prefix_icon, text }
}
