use muda::MenuId;
use serde::Deserialize;
use serde::Serialize;

use crate::error::BoxDynError;
use crate::prefs::BuiltinCollection;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsItem;
use crate::prefs::SortOrder;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AppCommand {
	// File menu
	FileExit,

	// Settings menu
	SettingsPaths,
	SettingsToggleBuiltinCollection(BuiltinCollection),

	// Help menu
	HelpRefreshInfoDb,
	HelpWebSite,
	HelpAbout,

	// Other
	Browse(PrefsCollection),
	HistoryAdvance(isize),
	SearchText(String),
	ItemsSort(usize, SortOrder),
	ItemsSelectedChanged,
	AddToExistingFolder(usize, Vec<PrefsItem>),
	AddToNewFolder(String, Vec<PrefsItem>),
	AddToNewFolderDialog(Vec<PrefsItem>),
	MoveCollection { old_index: usize, new_index: Option<usize> },
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
