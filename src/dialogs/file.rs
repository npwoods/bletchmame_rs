use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::Path;

use rfd::AsyncFileDialog;
use rfd::FileHandle;
use slint::ComponentHandle;

use crate::prefs::pathtype::PathType;
use crate::prefs::pathtype::PickType;

pub async fn choose_path_by_type_dialog(
	parent: &impl ComponentHandle,
	path_type: PathType,
	initial: Option<&Path>,
) -> Option<String> {
	// create the dialog and specify the initials
	let dialog = create_file_dialog(parent);
	let (initial_dir, initial_file) = initial_dir_and_file_from_path(initial);
	let dialog = set_file_dialog_initial_target(dialog, initial_dir, initial_file);

	let fh = match path_type.pick_type() {
		PickType::File { name, extension } => dialog.add_filter(name, &[extension]).pick_file().await,
		PickType::Dir => dialog.pick_folder().await,
	}?;

	Some(string_from_filehandle_lossy(fh))
}

pub async fn load_file_dialog(
	parent: &impl ComponentHandle,
	title: &str,
	file_types: &[(Option<&str>, &str)],
	initial_dir: Option<&Path>,
	initial_file: Option<&str>,
) -> Option<String> {
	let dialog = create_file_dialog(parent);
	let dialog = dialog.set_title(title);
	let dialog = set_file_dialog_file_types(dialog, file_types);
	let dialog = set_file_dialog_initial_target(dialog, initial_dir, initial_file);
	dialog.pick_file().await.map(string_from_filehandle_lossy)
}

pub async fn save_file_dialog(
	parent: &impl ComponentHandle,
	title: &str,
	file_types: &[(Option<&str>, &str)],
	initial_dir: Option<&Path>,
	initial_file: Option<&str>,
) -> Option<String> {
	let mut dialog = create_file_dialog(parent);
	dialog = dialog.set_title(title);
	dialog = set_file_dialog_file_types(dialog, file_types);
	dialog = set_file_dialog_initial_target(dialog, initial_dir, initial_file);
	dialog.save_file().await.map(string_from_filehandle_lossy)
}

fn create_file_dialog(parent: &impl ComponentHandle) -> AsyncFileDialog {
	AsyncFileDialog::new().set_parent(&parent.window().window_handle())
}

fn set_file_dialog_file_types(mut dialog: AsyncFileDialog, file_types: &[(Option<&str>, &str)]) -> AsyncFileDialog {
	for (desc, ext) in file_types {
		let desc = if let Some(desc) = desc.as_ref() {
			Cow::Borrowed(*desc)
		} else {
			Cow::Owned(format!("{} files", ext.to_uppercase()))
		};
		let name = format!("{desc} (*.{ext})");
		let extensions = [*ext];
		dialog = dialog.add_filter(name, &extensions);
	}
	dialog.add_filter("All Files (*.*)", &["*"])
}

pub fn initial_dir_and_file_from_path(initial: Option<&Path>) -> (Option<&'_ Path>, Option<&'_ str>) {
	if let Some(initial) = initial {
		let metadata = initial.metadata().ok();
		if metadata.as_ref().is_some_and(|x| x.is_dir()) {
			(Some(initial), None)
		} else if metadata.as_ref().is_some_and(|x| x.is_file()) {
			(initial.parent(), initial.file_name().and_then(OsStr::to_str))
		} else {
			(None, None)
		}
	} else {
		(None, None)
	}
}

fn set_file_dialog_initial_target(
	mut dialog: AsyncFileDialog,
	initial_dir: Option<&Path>,
	initial_file: Option<&str>,
) -> AsyncFileDialog {
	if let Some(initial_dir) = initial_dir {
		dialog = dialog.set_directory(initial_dir);
	}
	if let Some(initial_file) = initial_file {
		dialog = dialog.set_file_name(initial_file);
	}
	dialog
}

fn string_from_filehandle_lossy(fh: FileHandle) -> String {
	fh.path().to_string_lossy().into_owned()
}
