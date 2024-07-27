use std::fmt::Display;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use derive_enum_all_values::AllValues;
use num::clamp;
use serde::Deserialize;
use serde::Serialize;
use slint::LogicalSize;

use crate::Error;
use crate::Result;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
	#[serde(skip)]
	pub prefs_path: Option<PathBuf>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub paths: PrefsPaths,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub window_size: Option<PrefsSize>,

	#[serde(default)]
	pub collections: Vec<Rc<PrefsCollection>>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub history: Vec<HistoryEntry>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub history_position: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrefsPaths {
	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub mame_executable: String,
	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub software_lists: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrefsSize {
	pub width: f32,
	pub height: f32,
}

impl From<LogicalSize> for PrefsSize {
	fn from(value: LogicalSize) -> Self {
		Self {
			width: value.width,
			height: value.height,
		}
	}
}

impl From<PrefsSize> for LogicalSize {
	fn from(value: PrefsSize) -> Self {
		Self {
			width: value.width,
			height: value.height,
		}
	}
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PrefsCollection {
	Builtin(BuiltinCollection),
	MachineSoftware {
		#[serde(rename = "machine")]
		machine_name: String,
	},
	Folder {
		name: String,

		#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
		items: Vec<PrefsItem>,
	},
}

#[derive(AllValues, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", tag = "subtype")]
pub enum BuiltinCollection {
	All,
}

impl Display for BuiltinCollection {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let s = match self {
			BuiltinCollection::All => "All Systems",
		};
		write!(f, "{s}")
	}
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
	#[serde(flatten)]
	pub collection: Rc<PrefsCollection>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub selection: Vec<PrefsItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PrefsItem {
	Machine {
		#[serde(rename = "machine")]
		machine_name: String,
	},
}

const PREFS: Option<&str> = Some("BletchMAME.json");

impl Preferences {
	pub fn load(prefs_path: Option<impl AsRef<Path> + Copy>) -> Result<Self> {
		let path = prefs_filename(prefs_path, PREFS).map_err(prefs_load_error)?;
		let mut result = load_prefs(&path)?;
		result.prefs_path = prefs_path.map(|x| x.as_ref().to_path_buf());
		Ok(result)
	}

	pub fn save(&self) -> Result<()> {
		let path = prefs_filename(self.prefs_path.as_ref(), PREFS).map_err(prefs_save_error)?;
		save_prefs(self, &path)
	}

	pub fn fresh(prefs_path: Option<PathBuf>) -> Self {
		let json = include_str!("prefs_fresh.json");
		let mut result = load_prefs_from_reader(json.as_bytes()).unwrap();
		result.prefs_path = prefs_path;
		result
	}
}

pub fn prefs_filename(prefs_path: Option<impl AsRef<Path>>, filename: Option<&str>) -> Result<PathBuf> {
	let mut pathbuf = prefs_path
		.ok_or(Error::CantFindPreferencesDirectory)?
		.as_ref()
		.to_path_buf();
	if let Some(filename) = filename {
		pathbuf.push(filename);
	}
	Ok(pathbuf)
}

fn load_prefs(path: &Path) -> Result<Preferences> {
	let file = File::open(path).map_err(prefs_load_error)?;
	load_prefs_from_reader(file)
}

fn load_prefs_from_reader(reader: impl Read) -> Result<Preferences> {
	// deserialize the preferences
	let reader = BufReader::new(reader);
	let mut prefs: Preferences = serde_json::from_reader(reader).map_err(prefs_load_error)?;

	// special treatments
	if prefs.history.is_empty() {
		prefs.history = Preferences::fresh(None).history;
	}
	prefs.history_position = clamp(prefs.history_position, 0, prefs.history.len() - 1);

	// and return!
	Ok(prefs)
}

fn save_prefs(prefs: &Preferences, path: &Path) -> Result<()> {
	// only save if there is a change
	if load_prefs(path).ok().as_ref() != Some(prefs) {
		let mut file = File::create(path).map_err(prefs_save_error)?;
		let json = save_prefs_to_string(prefs)?;
		file.write_all(json.as_bytes()).map_err(prefs_save_error)?;
	}
	Ok(())
}

fn save_prefs_to_string(prefs: &Preferences) -> Result<String> {
	let json = serde_json::to_string_pretty(prefs).map_err(prefs_save_error)?;
	Ok(json)
}

fn prefs_load_error(e: impl std::error::Error + Send + Sync + 'static) -> Error {
	Error::PreferencesLoad(e.into())
}

fn prefs_save_error(e: impl std::error::Error + Send + Sync + 'static) -> Error {
	Error::PreferencesSave(e.into())
}

#[cfg(test)]
mod test {
	use super::load_prefs_from_reader;
	use super::save_prefs_to_string;
	use super::Preferences;

	#[test]
	pub fn test() {
		let prefs = Preferences::fresh(None);
		let json = save_prefs_to_string(&prefs).expect("Failed to save fresh prefs");

		let fresh_json = include_str!("prefs_fresh.json");
		assert_eq!(fresh_json.replace("\r", ""), json.replace("\r", ""));

		let new_prefs = load_prefs_from_reader(json.as_bytes()).expect("Failed to load saved fresh prefs");
		assert_eq!(prefs, new_prefs);
	}
}
