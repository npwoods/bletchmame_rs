use muda::MenuId;
use muda::MenuItem;
use serde::Deserialize;
use serde::Serialize;

use crate::error::BoxDynError;
use crate::guiutils::menuing::accel;
use crate::prefs::PrefsCollection;
use crate::prefs::SortOrder;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AppCommand {
	// File menu
	FileExit,

	// Help menu
	HelpRefreshInfoDb,
	HelpWebSite,

	// Other
	Browse(PrefsCollection),
	HistoryAdvance(isize),
	SearchText(String),
	ItemsSort(usize, SortOrder),
	ItemsSelectedChanged,
}

impl AppCommand {
	pub fn into_menu_item(self) -> MenuItem {
		let (text, enabled, accel_text) = match &self {
			AppCommand::FileExit => ("Exit", true, Some("Ctrl+Alt+X")),
			AppCommand::HelpRefreshInfoDb => ("Refresh MAME machine info...", false, None),
			AppCommand::HelpWebSite => ("BlechMAME web site...", true, None),
			AppCommand::Browse(_) => ("Browse", true, None),
			_ => panic!("into_menu_item() not supported for {:?}", self),
		};
		let accelerator = accel_text.and_then(accel);
		MenuItem::with_id(self, text, enabled, accelerator)
	}
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
