use std::sync::Arc;

use muda::MenuId;
use serde::Deserialize;
use serde::Serialize;

use crate::dialogs::file::PathType;
use crate::error::BoxDynError;
use crate::prefs::BuiltinCollection;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsItem;
use crate::prefs::SortOrder;
use crate::status::Update;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum AppCommand {
	// File menu
	FileStop,
	FilePause,
	FileExit,

	// Options menu
	OptionsThrottleRate(f32),
	OptionsToggleWarp,

	// Settings menu
	SettingsPaths,
	SettingsToggleBuiltinCollection(BuiltinCollection),
	SettingsReset,

	// Help menu
	HelpRefreshInfoDb,
	HelpWebSite,
	HelpAbout,

	// MAME communication
	MameSessionStarted,
	MameSessionEnded,
	MameStatusUpdate(Update),
	MamePing,
	ErrorMessageBox(String),

	// Other
	Shutdown,
	RunMame {
		machine_name: String,
		software_name: Option<Arc<str>>,
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
	ChoosePath(PathType),
	BookmarkCurrentCollection,
}

const MENU_PREFIX: &str = "MENU_";

impl From<AppCommand> for MenuId {
	fn from(value: AppCommand) -> Self {
		format!("{}{}", MENU_PREFIX, serde_json::to_string(&value).unwrap()).into()
	}
}

impl TryFrom<&MenuId> for AppCommand {
	type Error = BoxDynError;

	fn try_from(value: &MenuId) -> std::result::Result<Self, Self::Error> {
		let value = value
			.as_ref()
			.strip_prefix(MENU_PREFIX)
			.ok_or_else(|| BoxDynError::from("Not a menu string"))?;
		Ok(serde_json::from_str(value)?)
	}
}
