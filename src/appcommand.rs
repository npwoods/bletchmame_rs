use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;
use slint::SharedString;
use strum::EnumProperty;
use strum::IntoStaticStr;

use crate::prefs::BuiltinCollection;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsItem;
use crate::prefs::SortOrder;
use crate::prefs::pathtype::PathType;
use crate::runtime::command::MameCommand;
use crate::status::InputClass;
use crate::status::Update;
use crate::version::MameVersion;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, EnumProperty, IntoStaticStr)]
pub enum AppCommand {
	// File menu
	FileStop,
	FilePause,
	FileDevicesAndImages,
	FileQuickLoadState,
	FileQuickSaveState,
	FileLoadState,
	FileSaveState,
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
	SettingsInput(InputClass),
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
	IssueMameCommand(MameCommand),
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
	UnloadImage {
		tag: String,
	},
	ConnectToSocketDialog {
		tag: String,
	},
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

	pub fn encode_for_slint(&self) -> SharedString {
		format!("{}{}", MENU_PREFIX, serde_json::to_string(self).unwrap()).into()
	}

	pub fn decode_from_slint(s: SharedString) -> Option<Self> {
		(!s.is_empty()).then(|| {
			let json = s.strip_prefix(MENU_PREFIX).expect("not a menu string");
			serde_json::from_str(json).unwrap()
		})
	}
}

impl From<MameCommand> for AppCommand {
	fn from(value: MameCommand) -> Self {
		Self::IssueMameCommand(value)
	}
}
