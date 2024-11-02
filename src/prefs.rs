use std::borrow::Cow;
use std::fs;
use std::fs::rename;
use std::fs::File;
use std::io::BufReader;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Error;
use anyhow::Result;
use derive_enum_all_values::AllValues;
use num::clamp;
use serde::Deserialize;
use serde::Serialize;
use slint::LogicalSize;
use tracing::event;
use tracing::Level;

use crate::history::History;
use crate::icon::Icon;
use crate::info::InfoDb;

const LOG: Level = Level::DEBUG;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
	#[serde(skip)]
	pub prefs_path: Option<PathBuf>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub paths: Rc<PrefsPaths>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub window_size: Option<PrefsSize>,

	#[serde(default)]
	pub items_columns: Vec<PrefsColumn>,

	#[serde(default)]
	pub collections: Vec<Rc<PrefsCollection>>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub history: Vec<HistoryEntry>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub history_position: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrefsPaths {
	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub mame_executable: Option<String>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub roms: Vec<String>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub samples: Vec<String>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub plugins: Vec<String>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub software_lists: Vec<String>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub cfg: Option<String>,
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrefsColumn {
	#[serde(rename = "type")]
	pub column_type: ColumnType,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub sort: Option<SortOrder>,

	pub width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
	Ascending,
	Descending,
}

#[derive(AllValues, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum_macros::Display)]
#[serde(rename_all = "camelCase")]
pub enum ColumnType {
	#[strum(to_string = "Name")]
	Name,
	#[strum(to_string = "Source File")]
	SourceFile,
	#[strum(to_string = "Description")]
	Description,
	#[strum(to_string = "Year")]
	Year,
	#[strum(to_string = "Provider")]
	Provider,
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

impl PrefsCollection {
	pub fn icon(&self) -> Icon {
		match self {
			PrefsCollection::Builtin(_) | PrefsCollection::MachineSoftware { .. } => Icon::Search,
			PrefsCollection::Folder { .. } => Icon::Folder,
		}
	}

	pub fn description(&self, info_db: &InfoDb) -> Cow<'_, str> {
		match self {
			PrefsCollection::Builtin(x) => format!("{x}").into(),
			PrefsCollection::MachineSoftware { machine_name } => {
				let machine_desc = info_db.machines().find(machine_name).unwrap().description();
				format!("Software for \"{}\"", machine_desc).into()
			}
			PrefsCollection::Folder { name, items: _ } => Cow::Borrowed(name),
		}
	}
}

#[derive(AllValues, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, strum_macros::Display)]
#[serde(rename_all = "camelCase", tag = "subtype")]
pub enum BuiltinCollection {
	#[strum(to_string = "All Systems")]
	All,
	#[strum(to_string = "All Software")]
	AllSoftware,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
	#[serde(flatten)]
	pub collection: Rc<PrefsCollection>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub search: String,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub sort_suppressed: bool,

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
	Software {
		software_list: String,
		software: String,
		#[serde(rename = "machines")]
		machine_names: Vec<String>,
	},
}

const PREFS: Option<&str> = Some("BletchMAME.json");
const PREFS_BACKUP: Option<&str> = Some("BletchMAME.backup.json");

impl Preferences {
	pub fn load(prefs_path: Option<impl AsRef<Path> + Copy>) -> Result<Option<Self>> {
		// try to load the preferences
		let path = prefs_filename(prefs_path, PREFS)?;
		let result = load_prefs(&path);
		event!(LOG, "Preferences::load(): result={:?}", result.as_ref().map(|_| ()));

		// did we error?
		if result.is_err() {
			// we did; back up this file
			if let Ok(renamed) = prefs_filename(prefs_path, PREFS_BACKUP) {
				let rc = rename(&path, &renamed);
				event!(LOG, "Preferences::load(): {:?} ==> {:?}; rc={:?}", path, renamed, rc,);
			}
		}

		// store the prefs_path and return
		if let Ok(Some(mut result)) = result {
			result.prefs_path = prefs_path.map(|x| x.as_ref().to_path_buf());
			Ok(Some(result))
		} else {
			result
		}
	}

	pub fn save(&self) -> Result<()> {
		if let Some(prefs_path) = &self.prefs_path {
			ensure_directory(prefs_path);
		}
		let path = prefs_filename(self.prefs_path.as_ref(), PREFS)?;
		save_prefs(self, &path)
	}

	pub fn fresh(prefs_path: Option<PathBuf>) -> Self {
		let json = include_str!("prefs_fresh.json");
		let mut result = load_prefs_from_reader(json.as_bytes()).unwrap();
		Rc::get_mut(&mut result.paths).unwrap().cfg = prefs_path
			.as_ref()
			.and_then(|x| x.clone().into_os_string().into_string().ok());
		result.prefs_path = prefs_path;
		result
	}
}

pub fn prefs_filename(prefs_path: Option<impl AsRef<Path>>, filename: Option<&str>) -> Result<PathBuf> {
	let mut pathbuf = prefs_path
		.ok_or_else(|| Error::msg("Cannot find preferences directory"))?
		.as_ref()
		.to_path_buf();
	if let Some(filename) = filename {
		pathbuf.push(filename);
	}
	Ok(pathbuf)
}

fn load_prefs(path: &Path) -> Result<Option<Preferences>> {
	let file = match File::open(path) {
		Ok(x) => x,
		Err(e) => {
			return if e.kind() == ErrorKind::NotFound {
				Ok(None)
			} else {
				Err(Error::new(e))
			}
		}
	};

	let prefs = load_prefs_from_reader(file)?;
	Ok(Some(prefs))
}

fn load_prefs_from_reader(reader: impl Read) -> Result<Preferences> {
	// deserialize the preferences
	let reader = BufReader::new(reader);
	let mut prefs: Preferences =
		serde_json::from_reader(reader).map_err(|e| Error::new(e).context("Error loading preferences"))?;

	// special treatments
	prefs_treatments(&mut prefs);

	// and return!
	Ok(prefs)
}

/// special treatments to enforce variants
fn prefs_treatments(prefs: &mut Preferences) {
	// purge irrelevant history entries
	prefs.purge_stray_entries();

	// ensure that history is not empty
	if prefs.history.is_empty() {
		prefs.history = Preferences::fresh(None).history;
	}

	// enforce that history_position points to a valid entry
	prefs.history_position = clamp(prefs.history_position, 0, prefs.history.len() - 1);

	// enforce that we have at least one column
	if prefs.items_columns.is_empty() {
		prefs.items_columns = Preferences::fresh(None).items_columns;
		assert!(!prefs.items_columns.is_empty());
	}
}

fn save_prefs(prefs: &Preferences, path: &Path) -> Result<()> {
	// only save if there is a change
	if load_prefs(path).ok().flatten().as_ref() != Some(prefs) {
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

fn prefs_save_error(e: impl Into<Error>) -> Error {
	e.into().context("Error saving preferences")
}

fn ensure_directory(path: &impl AsRef<Path>) {
	if !fs::metadata(path).is_ok_and(|m| m.is_dir()) {
		let _ = fs::create_dir(path);
	}
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
		assert_eq!(fresh_json.replace('\r', ""), json.replace('\r', ""));

		let new_prefs = load_prefs_from_reader(json.as_bytes()).expect("Failed to load saved fresh prefs");
		assert_eq!(prefs, new_prefs);
	}
}
