use std::path::Path;

use derive_enum_all_values::AllValues;
use rfd::FileDialog;
use serde::Deserialize;
use serde::Serialize;
use slint::ComponentHandle;

const EXE_EXTENSION: &str = if cfg!(target_os = "windows") { "exe" } else { "" };

#[derive(
	AllValues, Clone, Copy, Debug, Default, strum_macros::Display, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
pub enum PathType {
	#[default]
	#[strum(to_string = "MAME Executable")]
	MameExecutable,
	#[strum(to_string = "Software Lists")]
	SoftwareLists,
}

impl PathType {
	pub fn is_multi(&self) -> bool {
		match self {
			Self::MameExecutable => false,
			Self::SoftwareLists => true,
		}
	}

	fn pick_type(&self) -> PickType {
		match self {
			Self::MameExecutable => PickType::File {
				name: "MAME Executable",
				extension: EXE_EXTENSION,
			},
			Self::SoftwareLists => PickType::Dir,
		}
	}

	pub fn path_exists(&self, path: impl AsRef<Path>) -> bool {
		std::fs::metadata(path)
			.map(|metadata| match self.pick_type() {
				PickType::File { .. } => metadata.is_file(),
				PickType::Dir => metadata.is_dir(),
			})
			.unwrap_or_default()
	}
}

enum PickType {
	File {
		name: &'static str,
		extension: &'static str,
	},
	Dir,
}

pub fn file_dialog(_parent: &impl ComponentHandle, path_type: PathType) -> Option<String> {
	let dialog = FileDialog::new();
	let path = match path_type.pick_type() {
		PickType::File { name, extension } => dialog.add_filter(name, &[extension]).pick_file(),
		PickType::Dir => dialog.pick_folder(),
	}?;

	// we have a `PathBuf`; we want a `String`; something is very messed up if this conversion fails
	path.into_os_string().into_string().ok()
}
