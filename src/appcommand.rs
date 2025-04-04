use std::sync::Arc;

use anyhow::Error;
use itertools::Itertools;
use muda::MenuId;
use serde::Deserialize;
use serde::Serialize;
use strum::EnumProperty;
use strum_macros::EnumProperty;

use crate::prefs::BuiltinCollection;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsItem;
use crate::prefs::SortOrder;
use crate::prefs::pathtype::PathType;
use crate::status::Update;
use crate::version::MameVersion;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, EnumProperty)]
pub enum AppCommand {
	// File menu
	FileStop,
	FilePause,
	FileDevicesAndImages,
	FileSaveScreenshot,
	#[strum(props(MinimumMame = "0.221"))]
	FileRecordMovie, // recording movies by specifying absolute paths was introduced in MAME 0.221
	FileDebugger,
	FileResetSoft,
	FileResetHard,
	FileExit,

	// Options menu
	OptionsThrottleRate(f32),
	OptionsToggleWarp,
	OptionsToggleFullScreen,
	OptionsToggleSound,
	#[strum(props(MinimumMame = "0.274"))]
	OptionsClassic,

	// Settings menu
	SettingsPaths(Option<PathType>),
	SettingsToggleBuiltinCollection(BuiltinCollection),
	SettingsReset,

	// Help menu
	HelpRefreshInfoDb,
	HelpWebSite,
	HelpAbout,

	// MAME communication
	MameSessionEnded,
	MameStatusUpdate(Update),
	ErrorMessageBox(String),

	// Other
	RunMame {
		machine_name: String,
		initial_loads: Vec<(Arc<str>, Arc<str>)>,
	},
	Browse(PrefsCollection),
	HistoryAdvance(isize),
	SearchText(String),
	ItemsSort(usize, SortOrder),
	ItemsSelectedChanged,
	AddToExistingFolder(usize, Vec<PrefsItem>),
	AddToNewFolder(String, Vec<PrefsItem>),
	AddToNewFolderDialog(Vec<PrefsItem>),
	RemoveFromFolder(String, Vec<PrefsItem>),
	MoveCollection {
		old_index: usize,
		new_index: Option<usize>,
	},
	DeleteCollectionDialog {
		index: usize,
	},
	RenameCollectionDialog {
		index: usize,
	},
	RenameCollection {
		index: usize,
		new_name: String,
	},
	BookmarkCurrentCollection,
	LoadImageDialog {
		tag: String,
	},
	LoadImage {
		tag: String,
		filename: String,
	},
	UnloadImage {
		tag: String,
	},
	ConnectToSocketDialog {
		tag: String,
	},
	ChangeSlots(Vec<(String, Option<String>)>),
	InfoDbBuildProgress {
		machine_description: String,
	},
	InfoDbBuildComplete,
	InfoDbBuildCancel,
	ReactivateMame,
	Configure {
		folder_name: String,
		index: usize,
	},
}

const MENU_PREFIX: &str = "MENU_";

impl AppCommand {
	pub fn minimum_mame_version(&self) -> Option<MameVersion> {
		let s = self.get_str("MinimumMame")?;
		let Some((Ok(major), Ok(minor))) = s.split('.').map(|s| s.parse()).collect_tuple() else {
			panic!("Failed to parse {s}");
		};
		Some(MameVersion::new(major, minor))
	}
}

impl From<AppCommand> for MenuId {
	fn from(value: AppCommand) -> Self {
		format!("{}{}", MENU_PREFIX, serde_json::to_string(&value).unwrap()).into()
	}
}

impl TryFrom<&MenuId> for AppCommand {
	type Error = Error;

	fn try_from(value: &MenuId) -> std::result::Result<Self, Self::Error> {
		let value = value
			.as_ref()
			.strip_prefix(MENU_PREFIX)
			.ok_or_else(|| Error::msg("Not a menu string"))?;
		Ok(serde_json::from_str(value)?)
	}
}
