use serde::Deserialize;
use serde::Serialize;
use strum::EnumIter;
use strum::EnumProperty;
use strum::EnumString;
use strum::VariantArray;

use crate::prefs::PathsStore;
use crate::prefs::access_paths;

const EXE_EXTENSION: &str = if cfg!(target_os = "windows") { "exe" } else { "" };

#[derive(
	EnumIter,
	VariantArray,
	Clone,
	Copy,
	Debug,
	Default,
	strum::Display,
	EnumString,
	EnumProperty,
	PartialEq,
	Eq,
	Hash,
	Serialize,
	Deserialize,
)]
pub enum PathType {
	#[default]
	#[strum(to_string = "MAME Executable")]
	MameExecutable,
	#[strum(to_string = "ROMs", props(MameArgument = "-rompath"))]
	Roms,
	#[strum(to_string = "Samples", props(MameArgument = "-samplepath"))]
	Samples,
	#[strum(to_string = "Software Lists", props(MameArgument = "-hashpath"))]
	SoftwareLists,
	#[strum(to_string = "Plugins", props(MameArgument = "-pluginspath"))]
	Plugins,
	#[strum(to_string = "MAME Configs", props(MameArgument = "-cfg_directory"))]
	Cfg,
	#[strum(to_string = "NVRAM", props(MameArgument = "-nvram_directory"))]
	Nvram,
	#[strum(to_string = "Cheats", props(MameArgument = "-cheatpath"))]
	Cheats,
	#[strum(to_string = "Snapshots")]
	Snapshots,
	#[strum(to_string = "History")]
	History,
}

impl PathType {
	pub fn is_multi(&self) -> bool {
		match access_paths(*self) {
			(_, PathsStore::Single(_)) => false,
			(_, PathsStore::Multiple(_)) => true,
		}
	}

	pub fn pick_type(&self) -> PickType {
		match self {
			Self::MameExecutable => PickType::File {
				name: "MAME Executable",
				extension: EXE_EXTENSION,
			},
			Self::History => PickType::File {
				name: "History XML",
				extension: "xml",
			},
			Self::Roms
			| Self::Samples
			| Self::SoftwareLists
			| Self::Plugins
			| Self::Cfg
			| Self::Nvram
			| Self::Cheats => PickType::Dir,
			Self::Snapshots => PickType::DirOrFile,
		}
	}

	pub fn mame_argument(&self) -> Option<&'static str> {
		self.get_str("MameArgument")
	}
}

pub enum PickType {
	File {
		name: &'static str,
		extension: &'static str,
	},
	Dir,
	DirOrFile,
}
