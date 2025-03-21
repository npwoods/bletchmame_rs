use std::ffi::OsStr;
use std::path::Path;

use rfd::FileDialog;
use slint::ComponentHandle;

use crate::prefs::pathtype::PathType;
use crate::prefs::pathtype::PickType;

pub fn file_dialog(_parent: &impl ComponentHandle, path_type: PathType, initial: Option<&Path>) -> Option<String> {
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
	let dialog = FileDialog::new();
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

	let path = match path_type.pick_type() {
		PickType::File { name, extension } => dialog.add_filter(name, &[extension]).pick_file(),
		PickType::Dir => dialog.pick_folder(),
	}?;

	// we have a `PathBuf`; we want a `String`; something is very messed up if this conversion fails
	path.into_os_string().into_string().ok()
}
