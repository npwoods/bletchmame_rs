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
	// determine the initial directory and/or file
	let (initial_directory, initial_file) = if let Some(initial) = initial {
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
	};

	// create the dialog and specify the initials
	let dialog = create_file_dialog(parent);
	let dialog = if let Some(initial_directory) = initial_directory {
		dialog.set_directory(initial_directory)
	} else {
		dialog
	};
	let dialog = if let Some(initial_file) = initial_file {
		dialog.set_file_name(initial_file)
	} else {
		dialog
	};

	let fh = match path_type.pick_type() {
		PickType::File { name, extension } => dialog.add_filter(name, &[extension]).pick_file().await,
		PickType::Dir => dialog.pick_folder().await,
	}?;

	Some(string_from_filehandle_lossy(fh))
}

pub async fn save_file_dialog(
	parent: &impl ComponentHandle,
	title: &str,
	file_types: &[(Option<&str>, &str)],
	initial_file: Option<String>,
) -> Option<String> {
	let mut dialog = create_file_dialog(parent);
	dialog = dialog.set_title(title);
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
	dialog = dialog.add_filter("All Files (*.*)", &["*"]);

	if let Some(initial_file) = initial_file {
		dialog = dialog.set_file_name(initial_file);
	}

	dialog.save_file().await.map(string_from_filehandle_lossy)
}

fn create_file_dialog(parent: &impl ComponentHandle) -> AsyncFileDialog {
	AsyncFileDialog::new().set_parent(&parent.window().window_handle())
}

fn string_from_filehandle_lossy(fh: FileHandle) -> String {
	fh.path().to_string_lossy().into_owned()
}
