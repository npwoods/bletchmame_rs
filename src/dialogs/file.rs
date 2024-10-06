use std::path::Path;

use derive_enum_all_values::AllValues;
use itertools::Itertools;
use rfd::FileDialog;
use serde::Deserialize;
use serde::Serialize;
use slint::ComponentHandle;

use crate::prefs::PrefsPaths;

const EXE_EXTENSION: &str = if cfg!(target_os = "windows") { "exe" } else { "" };

#[derive(
	AllValues, Clone, Copy, Debug, Default, strum_macros::Display, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
pub enum PathType {
	#[default]
	#[strum(to_string = "MAME Executable")]
	MameExecutable,
	#[strum(to_string = "ROMs")]
	Roms,
	#[strum(to_string = "Samples")]
	Samples,
	#[strum(to_string = "Software Lists")]
	SoftwareLists,
	#[strum(to_string = "Plugins")]
	Plugins,
}

impl PathType {
	pub fn is_multi(&self) -> bool {
		match self {
			Self::MameExecutable => false,
			Self::Roms | Self::Samples | Self::SoftwareLists | Self::Plugins => true,
		}
	}

	fn pick_type(&self) -> PickType {
		match self {
			Self::MameExecutable => PickType::File {
				name: "MAME Executable",
				extension: EXE_EXTENSION,
			},
			Self::Roms | Self::Samples | Self::SoftwareLists | Self::Plugins => PickType::Dir,
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

	pub fn load_from_prefs_paths(prefs_paths: &PrefsPaths, path_type: PathType) -> Vec<&String> {
		enum SingleOrMultiple<'a> {
			Single(&'a Option<String>),
			Multiple(&'a Vec<String>),
		}

		let target = match path_type {
			PathType::MameExecutable => SingleOrMultiple::Single(&prefs_paths.mame_executable),
			PathType::Roms => SingleOrMultiple::Multiple(&prefs_paths.roms),
			PathType::Samples => SingleOrMultiple::Multiple(&prefs_paths.samples),
			PathType::SoftwareLists => SingleOrMultiple::Multiple(&prefs_paths.software_lists),
			PathType::Plugins => SingleOrMultiple::Multiple(&prefs_paths.plugins),
		};

		match target {
			SingleOrMultiple::Single(x) => x.iter().collect(),
			SingleOrMultiple::Multiple(x) => x.iter().collect(),
		}
	}

	pub fn store_in_prefs_paths(
		prefs_paths: &mut PrefsPaths,
		path_type: PathType,
		paths_iter: impl Iterator<Item = String>,
	) {
		enum SingleOrMultiple<'a> {
			Single(&'a mut Option<String>),
			Multiple(&'a mut Vec<String>),
		}

		let target = match path_type {
			PathType::MameExecutable => SingleOrMultiple::Single(&mut prefs_paths.mame_executable),
			PathType::Roms => SingleOrMultiple::Multiple(&mut prefs_paths.roms),
			PathType::Samples => SingleOrMultiple::Multiple(&mut prefs_paths.samples),
			PathType::SoftwareLists => SingleOrMultiple::Multiple(&mut prefs_paths.software_lists),
			PathType::Plugins => SingleOrMultiple::Multiple(&mut prefs_paths.plugins),
		};

		match target {
			SingleOrMultiple::Single(x) => *x = paths_iter.at_most_one().map_err(|_| ()).unwrap(),
			SingleOrMultiple::Multiple(x) => *x = paths_iter.collect(),
		}
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
