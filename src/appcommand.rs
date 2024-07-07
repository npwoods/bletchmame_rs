use muda::MenuId;
use serde::Deserialize;
use serde::Serialize;

use crate::error::BoxDynError;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AppCommand {
	// File menu
	FileExit,

	// Options menu
	OptionsBuiltinCollections,

	// Help menu
	HelpRefreshInfoDb,
	HelpWebSite,

	// Other
	MoveCollection { path: Box<[usize]>, delta: Option<i8> },
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
