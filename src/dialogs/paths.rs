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
use slint::VecModel;
use slint::Weak;
use slint::spawn_local;
use tracing::Level;
use tracing::event;

use crate::dialogs::SingleResult;
use crate::dialogs::file::choose_path_by_type_dialog;
use crate::guiutils::modal::Modal;
use crate::icon::Icon;
use crate::prefs::PrefsPaths;
use crate::prefs::pathtype::PathType;
use crate::ui::MagicListViewItem;
use crate::ui::PathsDialog;

const LOG: Level = Level::INFO;

struct State {
	dialog_weak: Weak<PathsDialog>,
	paths: RefCell<PrefsPaths>,
	original_paths: Rc<PrefsPaths>,
}

impl Debug for State {
	fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		fmt.debug_map()
			.entry(&"paths", &self.paths)
			.entry(&"original_paths", &self.original_paths)
			.finish_non_exhaustive()
	}
}

pub async fn dialog_paths(
	parent: Weak<impl ComponentHandle + 'static>,
	paths: Rc<PrefsPaths>,
	path_type: Option<PathType>,
) -> Option<PrefsPaths> {
	// prepare the dialog
	let modal = Modal::new(&parent.unwrap(), || PathsDialog::new().unwrap());
	let single_result = SingleResult::default();
	let state = State {
		dialog_weak: modal.dialog().as_weak(),
		paths: RefCell::new((*paths).clone()),
		original_paths: paths,
	};
	let state = Rc::new(state);

	// set up the "path labels" combo box
	let path_labels = PathType::all_values()
		.iter()
		.map(|x| format!("{}", *x).into())
		.collect::<Vec<_>>();
	let path_labels = VecModel::from(path_labels);
	let path_labels = ModelRc::new(path_labels);
	modal.dialog().set_path_labels(path_labels);

	// set up the "ok" button
	let signaller = single_result.signaller();
	modal.dialog().on_ok_clicked(move || {
		signaller.signal(true);
	});

	// set up the "cancel" button
	let signaller = single_result.signaller();
	modal.dialog().on_cancel_clicked(move || {
		signaller.signal(false);
	});

	// set up the "browse" button
	let state_clone = state.clone();
	modal.dialog().on_browse_clicked(move || {
		let dialog = state_clone.dialog_weak.unwrap();
		browse_clicked(&dialog, &state_clone.paths);
		model_contents_changed(&state_clone);
	});

	// set up the "delete" button
	let state_clone = state.clone();
	modal.dialog().on_delete_clicked(move || {
		let dialog = state_clone.dialog_weak.unwrap();
		delete_clicked(&dialog);
		model_contents_changed(&state_clone);
	});

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(false);
		CloseRequestResponse::KeepWindowShown
	});

	// ensure paths entries are updated
	let state_clone = state.clone();
	let model: PathEntriesModel = PathEntriesModel::new(modal.dialog().as_weak(), move || {
		model_contents_changed(&state_clone);
	});
	let model = ModelRc::from(Rc::new(model));
	modal.dialog().set_path_entries(model);
	let state_clone = state.clone();
	modal.dialog().on_path_label_index_changed(move || {
		let dialog = state_clone.dialog_weak.unwrap();
		update_paths_entries(&dialog, &state_clone.paths.borrow());
	});

	// set the index on the path type dropdown
	let path_label_index = path_type
		.and_then(|path_type| PathType::all_values().iter().position(|&x| x == path_type))
		.unwrap_or_default();
	if path_label_index != 0 {
		let path_label_index = i32::try_from(path_label_index).unwrap();

		// workaround for https://github.com/slint-ui/slint/issues/7632; please remove hack when fixed
		let dialog_weak = modal.dialog().as_weak();
		let state_clone = state.clone();
		let fut = async move {
			tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
			let dialog = dialog_weak.unwrap();
			dialog.set_path_label_index(path_label_index);
			update_paths_entries(&dialog, &state_clone.paths.borrow());
		};
		spawn_local(fut).unwrap();
	} else {
		// if `path_label_index` is zero, we don't have to do the above buffoonery
		update_paths_entries(modal.dialog(), &state.paths.borrow());
	}

	// update buttons when selected entries changes
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_path_entries_index_changed(move || {
		let dialog = dialog_weak.unwrap();
		update_buttons(&dialog);
	});

	// present the modal dialog
	let accepted = modal.run(async { single_result.wait().await }).await;

	// if the user hit "ok", return
	accepted.then(|| Rc::try_unwrap(state).unwrap().paths.into_inner())
}

fn path_type(dialog: &PathsDialog) -> PathType {
	dialog
		.get_path_label_index()
		.try_into()
		.ok()
		.and_then(|x: usize| PathType::all_values().get(x))
		.cloned()
		.unwrap_or_default()
}

fn update_paths_entries(dialog: &PathsDialog, paths: &PrefsPaths) {
	let path_type = path_type(dialog);

	let path_entries = paths.by_type(path_type);
	let paths_entries = path_entries
		.iter()
		.map(|path| {
			let exists = paths.path_exists(path_type, path);
			let prefix_icon = if exists { Icon::Blank } else { Icon::Clear };
			let path = SharedString::from(path);
			(path, prefix_icon)
		})
		.collect::<Vec<_>>();

	let model = dialog.get_path_entries();
	let model = PathEntriesModel::get_model(&model);
	model.update(paths_entries, path_type.is_multi());
}

fn browse_clicked(dialog: &PathsDialog, paths: &RefCell<PrefsPaths>) {
	let path_type = path_type(dialog);
	let model = dialog.get_path_entries();
	let model = PathEntriesModel::get_model(&model);

	// determine the existing path that should serve as the initial path for the dialog
	let existing_path = usize::try_from(dialog.get_path_entry_index())
		.ok()
		.and_then(|index| model.entry(index));
	let resolved_existing_path = existing_path.as_ref().and_then(|path| paths.borrow().resolve(path));
	let resolved_existing_path = resolved_existing_path.as_deref();
	event!(
		LOG,
		"browse_clicked(): existing_path={existing_path:?} resolved_existing_path={resolved_existing_path:?}"
	);

	// show the file dialog
	let Some(path) = choose_path_by_type_dialog(dialog, path_type, resolved_existing_path) else {
		return;
	};
	let Ok(row) = usize::try_from(dialog.get_path_entry_index()) else {
		return;
	};
	model.set_entry(row, &path, Icon::Blank);
}

fn delete_clicked(dialog: &PathsDialog) {
	let Ok(row) = usize::try_from(dialog.get_path_entry_index()) else {
		return;
	};
	let model = dialog.get_path_entries();
	let model = PathEntriesModel::get_model(&model);
	model.remove(row);
}

fn update_buttons(dialog: &PathsDialog) {
	let model = dialog.get_path_entries();
	let model = PathEntriesModel::get_model(&model);

	let row = usize::try_from(dialog.get_path_entry_index()).ok();
	dialog.set_browse_enabled(row.is_some());
	dialog.set_delete_enabled(row.is_some_and(|x| x < model.entry_count()));
}

fn model_contents_changed(state: &State) {
	let dialog = state.dialog_weak.unwrap();
	let mut paths = state.paths.borrow_mut();
	let original_paths = &state.original_paths;
	let model = dialog.get_path_entries();
	let model = PathEntriesModel::get_model(&model);

	let path_type = path_type(&dialog);
	let entries_iter = model.entries().into_iter().map(|x| x.to_string());
	paths.set_by_type(path_type, entries_iter);
	dialog.set_ok_enabled(*paths != **original_paths);
}

fn assign_if_changed<T>(target: &mut T, source: T) -> bool
where
	T: PartialEq,
{
	let changed = *target != source;
	if changed {
		*target = source;
	}
	changed
}

struct PathEntriesModel {
	dialog_weak: Weak<PathsDialog>,
	changed_func: Box<dyn Fn() + 'static>,
	data: RefCell<(Vec<(SharedString, Icon)>, bool)>,
	notify: ModelNotify,
}

impl PathEntriesModel {
	pub fn new(dialog_weak: Weak<PathsDialog>, changed_func: impl Fn() + 'static) -> Self {
		let changed_func = Box::new(changed_func) as Box<dyn Fn() + 'static>;
		let data = RefCell::new((Vec::new(), false));
		let notify = ModelNotify::default();
		Self {
			dialog_weak,
			changed_func,
			data,
			notify,
		}
	}

	pub fn update(&self, items: Vec<(SharedString, Icon)>, is_multi: bool) {
		self.data.replace((items, is_multi));
		self.notify.reset();
	}

	pub fn entry_count(&self) -> usize {
		self.data.borrow().0.len()
	}

	pub fn append_row_index(&self) -> Option<usize> {
		let data = self.data.borrow();
		let (entries, is_multi) = &*data;
		(*is_multi || entries.is_empty()).then_some(entries.len())
	}

	pub fn remove(&self, row: usize) {
		let mut data = self.data.borrow_mut();
		data.0.remove(row);
		self.notify.row_removed(row, 1);
	}

	pub fn entries(&self) -> Vec<SharedString> {
		let data = self.data.borrow();
		data.0.iter().map(|(s, _)| s.clone()).collect()
	}

	pub fn entry(&self, index: usize) -> Option<SharedString> {
		self.data.borrow().0.get(index).cloned().map(|x| x.0)
	}

	pub fn set_entry(&self, row: usize, text: impl Into<SharedString>, prefix_icon: Icon) {
		let new_value = (text.into(), prefix_icon);
		let changed = if self.append_row_index() == Some(row) {
			self.data.borrow_mut().0.push(new_value);
			self.notify.row_added(row, 1);
			true
		} else {
			let changed = assign_if_changed(&mut self.data.borrow_mut().0[row], new_value);
			if changed {
				self.notify.row_changed(row);
			}
			changed
		};
		if changed {
			(self.changed_func)();
		}
	}

	fn make_entry(&self, text: impl Into<SharedString>, prefix_icon: Icon) -> MagicListViewItem {
		let prefix_icon = prefix_icon.slint_icon(&self.dialog_weak.unwrap());
		let text = text.into();
		MagicListViewItem {
			prefix_icon,
			text,
			supporting_text: Default::default(),
		}
	}

	pub fn get_model(model: &ModelRc<MagicListViewItem>) -> &Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for PathEntriesModel {
	type Data = MagicListViewItem;

	fn row_count(&self) -> usize {
		let len = self.data.borrow().0.len();
		len + if self.append_row_index().is_some() { 1 } else { 0 }
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let (text, prefix_icon) = if self.append_row_index() == Some(row) {
			("<          >".into(), Icon::Blank)
		} else {
			self.data.borrow().0.get(row)?.clone()
		};
		let data = self.make_entry(text, prefix_icon);
		Some(data)
	}

	fn set_row_data(&self, row: usize, data: Self::Data) {
		self.set_entry(row, data.text, Icon::Blank);
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}
