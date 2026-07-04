pub mod args;
pub mod command;
pub mod session;
mod watchdog;

use serde::Deserialize;
use serde::Serialize;
use smol_str::SmolStr;
use strum::EnumString;

use crate::imagedesc::ImageDesc;
use crate::prefs::PrefsVideo;

#[derive(Clone, Debug)]
pub enum MameWindowing {
	Attached(SmolStr),
	Windowed,
	WindowedMaximized,
	Fullscreen,
}

#[derive(Clone, Copy, Debug, Default, EnumString)]
pub enum MameStderr {
	#[default]
	#[strum(ascii_case_insensitive)]
	Capture,
	#[strum(ascii_case_insensitive)]
	Inherit,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MameStartArgs {
	pub machine_name: String,
	pub ram_size: Option<u64>,
	pub bios: Option<String>,
	pub slots: Vec<(SmolStr, SmolStr)>,
	pub images: Vec<(SmolStr, ImageDesc)>,
	pub video: Option<PrefsVideo>,
}
