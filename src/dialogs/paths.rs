use std::cell::RefCell;
use std::default::Default;
use std::rc::Rc;

use derive_enum_all_values::AllValues;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;

use crate::dialogs::SingleResult;
use crate::guiutils::windowing::with_modal_parent;
use crate::prefs::PrefsPaths;
use crate::ui::PathsDialog;

#[derive(AllValues, Clone, Copy, Debug, Default, strum_macros::Display, PartialEq, Eq, Hash)]
enum PathType {
	#[default]
	#[strum(to_string = "MAME Executable")]
	MameExecutable,
	#[strum(to_string = "Software Lists")]
	SoftwareLists,
}

pub async fn dialog_paths(parent: Weak<impl ComponentHandle + 'static>, paths: PrefsPaths) -> Option<PrefsPaths> {
	let paths = Rc::new(RefCell::new(paths));

	// prepare the dialog
	let dialog = with_modal_parent(&parent.unwrap(), || PathsDialog::new().unwrap());
	let single_result = SingleResult::default();

	// set up the "path labels" combo box
	let path_labels = PathType::all_values()
		.iter()
		.map(|x| format!("{}", *x).into())
		.collect::<Vec<_>>();
	let path_labels = VecModel::from(path_labels);
	let path_labels = ModelRc::new(path_labels);
	dialog.set_path_labels(path_labels);

	// set up the "ok" button
	let signaller = single_result.signaller();
	dialog.on_ok_clicked(move || {
		signaller.signal(true);
	});

	// set up the "cancel" button
	let signaller = single_result.signaller();
	dialog.on_cancel_clicked(move || {
		signaller.signal(false);
	});

	// set up the close handler
	let signaller = single_result.signaller();
	dialog.window().on_close_requested(move || {
		signaller.signal(false);
		CloseRequestResponse::HideWindow
	});

	// update entries
	update_paths_entries(&dialog, &paths);

	// show the dialog and wait for completion
	dialog.show().unwrap();
	let accepted = single_result.wait().await;
	dialog.hide().unwrap();
	accepted.then(|| Rc::unwrap_or_clone(paths).into_inner())
}

fn update_paths_entries(dialog: &PathsDialog, paths: &Rc<RefCell<PrefsPaths>>) {
	let paths = paths.borrow();

	let path_type = dialog
		.get_path_label_index()
		.try_into()
		.ok()
		.and_then(|x: usize| PathType::all_values().get(x))
		.cloned()
		.unwrap_or_default();

	let path_entries = match path_type {
		// MAME Executable
		PathType::MameExecutable => (!paths.mame_executable.is_empty())
			.then_some(&paths.mame_executable)
			.into_iter()
			.collect::<Vec<_>>(),

		// Software Lists
		PathType::SoftwareLists => paths.software_lists.iter().collect::<Vec<_>>(),
	};
	let path_entries = path_entries
		.into_iter()
		.map(|text| SharedString::from(text).into())
		.collect::<Vec<_>>();
	let path_entries = VecModel::from(path_entries);
	let path_entries = ModelRc::new(path_entries);
	dialog.set_path_entries(path_entries);
}
