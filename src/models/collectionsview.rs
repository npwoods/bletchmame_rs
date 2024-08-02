use std::any::Any;
use std::cell::Cell;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use slint::spawn_local;
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
	after_refresh_callback: Cell<Option<Box<dyn Future<Output = ()> + 'static>>>,
	notify: ModelNotify,
}

impl CollectionsViewModel {
	pub fn new(app_window_weak: Weak<AppWindow>) -> Self {
		Self {
			app_window_weak,
			items: RefCell::new(Vec::new()),
			after_refresh_callback: Cell::new(None),
			notify: ModelNotify::default(),
		}
	}

	pub fn update(&self, items: &[Rc<PrefsCollection>]) {
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
}

impl Model for CollectionsViewModel {
	type Data = MagicListViewItem;

	fn row_count(&self) -> usize {
		invoke_after_refresh_callback(&self.after_refresh_callback);
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

fn invoke_after_refresh_callback(after_refresh_callback: &Cell<Option<Box<dyn Future<Output = ()> + 'static>>>) {
	if let Some(callback) = after_refresh_callback.take() {
		let callback = Pin::from(callback);
		spawn_local(callback).unwrap();
	}
}
