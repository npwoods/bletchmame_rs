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
	#[strum(to_string = "MAME Configs")]
	Cfg,
	#[strum(to_string = "NVRAM")]
	Nvram,
}

impl PathType {
	pub fn is_multi(&self) -> bool {
		match self.access() {
			(_, PathsStore::Single(_)) => false,
			(_, PathsStore::Multiple(_)) => true,
		}
	}

	fn pick_type(&self) -> PickType {
		match self {
			Self::MameExecutable => PickType::File {
				name: "MAME Executable",
				extension: EXE_EXTENSION,
			},
			Self::Roms | Self::Samples | Self::SoftwareLists | Self::Plugins | Self::Cfg | Self::Nvram => PickType::Dir,
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
		let (retrieve, _) = path_type.access();
		retrieve(prefs_paths).iter().collect()
	}

	pub fn store_in_prefs_paths(
		prefs_paths: &mut PrefsPaths,
		path_type: PathType,
		paths_iter: impl Iterator<Item = String>,
	) {
		let (_, store) = path_type.access();

		match store {
			PathsStore::Single(store) => {
				*store(prefs_paths) = paths_iter.at_most_one().map_err(|_| ()).unwrap();
			}
			PathsStore::Multiple(store) => {
				*store(prefs_paths) = paths_iter.collect();
			}
		}
	}

	fn access(&self) -> (fn(&PrefsPaths) -> &[String], PathsStore) {
		match self {
			PathType::MameExecutable => (
				(|x| x.mame_executable.as_slice()),
				PathsStore::Single(|x| &mut x.mame_executable),
			),
			PathType::Roms => ((|x| &x.roms), PathsStore::Multiple(|x| &mut x.roms)),
			PathType::Samples => ((|x| &x.samples), PathsStore::Multiple(|x| &mut x.samples)),
			PathType::SoftwareLists => ((|x| &x.software_lists), PathsStore::Multiple(|x| &mut x.software_lists)),
			PathType::Plugins => ((|x| &x.plugins), PathsStore::Multiple(|x| &mut x.plugins)),
			PathType::Cfg => ((|x| x.cfg.as_slice()), PathsStore::Single(|x| &mut x.cfg)),
			PathType::Nvram => ((|x| x.nvram.as_slice()), PathsStore::Single(|x| &mut x.nvram)),
		}
	}
}

#[derive(Debug)]
enum PathsStore {
	Single(fn(&mut PrefsPaths) -> &mut Option<String>),
	Multiple(fn(&mut PrefsPaths) -> &mut Vec<String>),
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
