use std::any::Any;
use std::cell::RefCell;
use std::default::Default;
use std::fmt::Debug;
use std::rc::Rc;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::SharedString;
use slint::ToSharedString;
use slint::VecModel;
use slint::Weak;
use slint::spawn_local;
use strum::IntoEnumIterator;
use strum::VariantArray;
use tokio::sync::mpsc;
use tracing::info;

use crate::dialogs::SenderExt;
use crate::dialogs::file::choose_path_by_type_dialog;
use crate::guiutils::modal::ModalStack;
use crate::prefs::PrefsPaths;
use crate::prefs::pathtype::PathType;
use crate::ui::PathsDialog;
use crate::ui::PathsListViewItem;

struct State {
	dialog_weak: Weak<PathsDialog>,
	paths: RefCell<PrefsPaths>,
	original_paths: Rc<PrefsPaths>,
}

struct PathEntriesModel {
	state: Rc<State>,
	data: RefCell<Vec<PathsListViewItem>>,
	notify: ModelNotify,
}

pub async fn dialog_paths(
	modal_stack: ModalStack,
	paths: Rc<PrefsPaths>,
	path_type: Option<PathType>,
) -> Option<PrefsPaths> {
	// prepare the dialog
	let modal = modal_stack.modal(|| PathsDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);
	let dialog_weak = modal.dialog().as_weak();
	let state = State::new(dialog_weak, paths);
	let state = Rc::new(state);

	// set up the "path labels" combo box
	let path_labels = PathType::iter().map(|x| x.to_shared_string()).collect::<Vec<_>>();
	let path_labels = VecModel::from(path_labels);
	let path_labels = ModelRc::new(path_labels);
	modal.dialog().set_path_labels(path_labels);

	// set up the "ok" button
	let tx_clone = tx.clone();
	modal.dialog().on_ok_clicked(move || {
		tx_clone.signal(true);
	});

	// set up the "cancel" button
	let tx_clone = tx.clone();
	modal.dialog().on_cancel_clicked(move || {
		tx_clone.signal(false);
	});

	// set up the "browse" button
	let state_clone = state.clone();
	modal.dialog().on_browse_clicked(move || {
		let state_clone = state_clone.clone();
		let fut = async move {
			state_clone.browse_clicked().await;
		};
		spawn_local(fut).unwrap();
	});

	// set up the "insert" button
	let state_clone = state.clone();
	modal.dialog().on_insert_clicked(move || {
		let state_clone = state_clone.clone();
		let fut = async move {
			state_clone.insert_clicked().await;
		};
		spawn_local(fut).unwrap();
	});

	// set up the "delete" button
	let state_clone = state.clone();
	modal.dialog().on_delete_clicked(move || {
		let state_clone = state_clone.clone();
		let fut = async move {
			state_clone.delete_clicked().await;
		};
		spawn_local(fut).unwrap();
	});

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(false);
		CloseRequestResponse::KeepWindowShown
	});

	// set up the paths entries model
	let model = PathEntriesModel::new(state.clone());
	let model = Rc::new(model);
	let model = ModelRc::from(model);
	modal.dialog().set_path_entries(model);

	// respond to path label index changed events
	let state_clone = state.clone();
	modal.dialog().on_path_label_index_changed(move || {
		state_clone.update_dialog_from_paths();
	});

	// determine the index on the paths dropdown
	let path_label_index = path_type
		.and_then(|path_type| PathType::iter().position(|x| x == path_type))
		.unwrap_or_default();
	if path_label_index > 0 {
		// workaround for https://github.com/slint-ui/slint/issues/7632; please remove hack when fixed
		let state_clone = state.clone();
		let fut = async move {
			tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
			let dialog = state_clone.dialog_weak.unwrap();
			let path_label_index = path_label_index.try_into().unwrap();
			dialog.set_path_label_index(path_label_index);
			state_clone.update_dialog_from_paths();
		};
		spawn_local(fut).unwrap();
	} else {
		// if `path_label_index` is zero, we don't have to do the above buffoonery
		state.update_dialog_from_paths();
	}

	// present the modal dialog
	let accepted = modal.run(async { rx.recv().await.unwrap() }).await;

	// if the user hit "ok", return
	accepted.then(|| Rc::try_unwrap(state).unwrap().paths.into_inner())
}

impl State {
	pub fn new(dialog_weak: Weak<PathsDialog>, paths: Rc<PrefsPaths>) -> Self {
		let original_paths = paths;
		let paths = RefCell::new((*original_paths).clone());
		Self {
			dialog_weak,
			paths,
			original_paths,
		}
	}

	pub fn update_dialog_from_paths(&self) {
		// get the current path type
		let path_type = self.current_path_type();

		// build the entries from the paths
		let mut entries = {
			let paths = self.paths.borrow();
			paths
				.by_type(path_type)
				.iter()
				.map(|text| {
					let text = text.to_shared_string();
					let is_error = !paths.path_exists(path_type, &text);
					PathsListViewItem { text, is_error }
				})
				.collect::<Vec<_>>()
		};

		// add the final "append" entry if appropriate
		if path_type.is_multi() || entries.is_empty() {
			let text = "".into();
			let is_error = false;
			entries.push(PathsListViewItem { text, is_error });
		}

		// update the entries from our paths variable
		self.with_path_entries_model(|model| model.set_entries(entries));

		// "ok" is enabled if things are different from the original
		let is_dirty = *self.paths.borrow() != *self.original_paths;
		self.dialog_weak.unwrap().set_ok_enabled(is_dirty);
	}

	pub fn update_paths_from_entries(&self) {
		let dialog = self.dialog_weak.unwrap();
		let path_type = PathType::VARIANTS[usize::try_from(dialog.get_path_label_index()).unwrap()];

		self.with_path_entries_model(|model| {
			model.with_entries(|entries| {
				let paths_iter = entries
					.iter()
					.filter(|x| !x.text.is_empty())
					.map(|x| x.text.as_str().into());
				self.paths.borrow_mut().set_by_type(path_type, paths_iter);
			});
		});
		self.update_dialog_from_paths();
	}

	pub async fn browse_clicked(&self) {
		self.dialog_weak.unwrap().invoke_stop_editing();
		let path_type = self.current_path_type();
		let path_entry_index = self.current_path_entry_index().unwrap();

		// determine the existing path that should serve as the initial path for the dialog
		let existing_path =
			self.with_path_entries_model(|model| model.with_entries(|entries| entries[path_entry_index].text.clone()));

		let resolved_existing_path = self.paths.borrow().resolve(&existing_path);
		let resolved_existing_path = resolved_existing_path.as_deref();
		info!(
			existing_path=?existing_path,
			resolved_existing_path=?resolved_existing_path,
			"browse_clicked()"
		);

		// show the file dialog
		let parent = self.dialog_weak.unwrap().window().window_handle();
		let Some(path) = choose_path_by_type_dialog(parent, path_type, resolved_existing_path).await else {
			return;
		};

		self.with_path_entries_model(|model| model.set_entry(path_entry_index, path));
	}

	pub async fn insert_clicked(&self) {
		self.dialog_weak.unwrap().invoke_stop_editing();
		let row = self.current_path_entry_index().unwrap();
		self.with_path_entries_model(|model| model.insert(row));
		self.dialog_weak.unwrap().invoke_begin_editing();
	}

	pub async fn delete_clicked(&self) {
		self.dialog_weak.unwrap().invoke_stop_editing();
		let row = self.current_path_entry_index().unwrap();
		self.with_path_entries_model(|model| model.remove(row));
	}

	fn current_path_type(&self) -> PathType {
		let dialog = self.dialog_weak.unwrap();
		PathType::VARIANTS[usize::try_from(dialog.get_path_label_index()).unwrap()]
	}

	fn current_path_entry_index(&self) -> Option<usize> {
		self.dialog_weak.unwrap().get_path_entry_index().try_into().ok()
	}

	fn with_path_entries_model<R>(&self, callback: impl FnOnce(&PathEntriesModel) -> R) -> R {
		let dialog = self.dialog_weak.unwrap();
		let model = dialog.get_path_entries();
		let model = PathEntriesModel::get_model(&model);
		callback(model)
	}
}

impl Debug for State {
	fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		fmt.debug_map()
			.entry(&"paths", &self.paths)
			.entry(&"original_paths", &self.original_paths)
			.finish_non_exhaustive()
	}
}

impl PathEntriesModel {
	pub fn new(state: Rc<State>) -> Self {
		let data = RefCell::new(Vec::new());
		let notify = ModelNotify::default();
		Self { state, data, notify }
	}

	pub fn with_entries<R>(&self, callback: impl FnOnce(&[PathsListViewItem]) -> R) -> R {
		let entries = self.data.borrow();
		callback(&entries)
	}

	pub fn set_entries(&self, entries: Vec<PathsListViewItem>) {
		if *self.data.borrow() != entries {
			self.data.replace(entries);
			self.notify.reset();
		}
	}

	pub fn set_entry(&self, row: usize, text: impl Into<SharedString>) {
		self.data.borrow_mut()[row].text = text.into();
		self.notify.row_changed(row);
		self.state.update_paths_from_entries();
	}

	pub fn insert(&self, row: usize) {
		self.data.borrow_mut().insert(row, Default::default());
		self.notify.row_added(row, 1);
	}

	pub fn remove(&self, row: usize) {
		self.data.borrow_mut().remove(row);
		self.notify.row_removed(row, 1);
		self.state.update_paths_from_entries();
	}

	pub fn get_model(model: &ModelRc<PathsListViewItem>) -> &Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for PathEntriesModel {
	type Data = PathsListViewItem;

	fn row_count(&self) -> usize {
		self.data.borrow().len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		self.data.borrow().get(row).cloned()
	}

	fn set_row_data(&self, row: usize, data: Self::Data) {
		self.data.borrow_mut()[row] = data;
		self.notify.row_changed(row);
		self.state.update_paths_from_entries();
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}
