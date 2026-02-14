pub mod pathtype;
mod preflight;
mod serde_slots;
mod var;

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::fs::create_dir_all;
use std::fs::rename;
use std::io::BufReader;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;

use anyhow::Error;
use anyhow::Result;
use itertools::Itertools;
use num::clamp;
use serde::Deserialize;
use serde::Serialize;
use slint::LogicalSize;
use slint::ToSharedString;
use smol_str::SmolStr;
use strum::EnumIter;
use strum::EnumProperty;
use strum::EnumString;
use tracing::info;

use crate::history::History;
use crate::icon::Icon;
use crate::imagedesc::ImageDesc;
use crate::info::InfoDb;
use crate::prefs::pathtype::PathType;
use crate::prefs::pathtype::PickType;
use crate::prefs::preflight::preflight_checks;
use crate::prefs::var::resolve_path;
use crate::prefs::var::resolve_paths_string;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[serde(default)]
pub struct Preferences {
	#[serde(skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub paths: Rc<PrefsPaths>,

	#[serde(skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub window_size: Option<PrefsSize>,

	#[serde(skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub main_window_left_column_width: Option<f32>,

	#[serde(skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub main_window_right_column_width: Option<f32>,

	pub is_fullscreen: bool,

	#[serde(skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub fullscreen_display: Option<SmolStr>,

	#[serde(flatten)]
	pub video: PrefsVideo,

	pub items_columns: Vec<PrefsColumn>,

	pub collections: Vec<Rc<PrefsCollection>>,

	#[serde(skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub history: Vec<HistoryEntry>,

	#[serde(skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub history_position: usize,

	pub show_stop_warning: bool,
}

impl Default for Preferences {
	fn default() -> Self {
		Self {
			paths: PrefsPaths::default().into(),
			window_size: None,
			main_window_left_column_width: None,
			main_window_right_column_width: None,
			is_fullscreen: false,
			fullscreen_display: None,
			video: Default::default(),
			items_columns: [].into(),
			collections: [].into(),
			history: [].into(),
			history_position: 0,
			show_stop_warning: true,
		}
	}
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrefsPaths {
	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub mame_executable: Option<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub roms: Vec<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub samples: Vec<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub plugins: Vec<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub software_lists: Vec<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub cfg: Option<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub diff: Option<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub inis: Vec<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub nvram: Option<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub cheats: Option<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub snapshots: Vec<SmolStr>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub history_file: Option<SmolStr>,
}

impl PrefsPaths {
	pub fn by_type(&self, path_type: PathType) -> &[SmolStr] {
		access_paths(path_type).0(self)
	}

	pub fn set_by_type(&mut self, path_type: PathType, paths_iter: impl Iterator<Item = SmolStr>) {
		let (_, store) = access_paths(path_type);
		match store {
			PathsStore::Single(store) => {
				*store(self) = paths_iter.at_most_one().map_err(|e| e.to_string()).unwrap();
			}
			PathsStore::Multiple(store) => {
				*store(self) = paths_iter.collect();
			}
		}
	}

	pub fn resolve<'a>(&self, path: &'a str) -> Option<Cow<'a, Path>> {
		resolve_path(path, self.mame_executable.as_deref())
	}

	pub fn full_string(&self, path_type: PathType) -> Option<OsString> {
		let paths = self.by_type(path_type);
		resolve_paths_string(paths, self.mame_executable.as_deref())
	}

	pub fn path_exists(&self, path_type: PathType, path: &str) -> bool {
		self.resolve(path)
			.and_then(|path| std::fs::metadata(path.as_ref()).ok())
			.is_some_and(|metadata| match path_type.pick_type() {
				PickType::File { .. } => metadata.is_file(),
				PickType::Dir => metadata.is_dir(),
				PickType::DirOrFile => metadata.is_file() || metadata.is_dir(),
			})
	}

	pub fn preflight(&self, skip_file_system_checks: bool) -> Vec<PreflightProblem> {
		let mame_executable_path = self.mame_executable.as_ref().and_then(|path| self.resolve(path));
		let mame_executable_path = mame_executable_path.as_ref().map(|path| path.as_ref());
		let plugins_path_iter = self.plugins.iter().flat_map(|path| self.resolve(path.as_ref()));
		preflight_checks(mame_executable_path, plugins_path_iter, skip_file_system_checks)
	}
}

#[derive(Debug)]
enum PathsStore {
	Single(fn(&mut PrefsPaths) -> &mut Option<SmolStr>),
	Multiple(fn(&mut PrefsPaths) -> &mut Vec<SmolStr>),
}

fn access_paths(path_type: PathType) -> (fn(&PrefsPaths) -> &[SmolStr], PathsStore) {
	match path_type {
		PathType::MameExecutable => (
			(|x| x.mame_executable.as_slice()),
			PathsStore::Single(|x| &mut x.mame_executable),
		),
		PathType::Roms => ((|x| &x.roms), PathsStore::Multiple(|x| &mut x.roms)),
		PathType::Samples => ((|x| &x.samples), PathsStore::Multiple(|x| &mut x.samples)),
		PathType::SoftwareLists => ((|x| &x.software_lists), PathsStore::Multiple(|x| &mut x.software_lists)),
		PathType::Plugins => ((|x| &x.plugins), PathsStore::Multiple(|x| &mut x.plugins)),
		PathType::Cfg => ((|x| x.cfg.as_slice()), PathsStore::Single(|x| &mut x.cfg)),
		PathType::Diff => ((|x| x.diff.as_slice()), PathsStore::Single(|x| &mut x.diff)),
		PathType::Ini => ((|x| &x.inis), PathsStore::Multiple(|x| &mut x.inis)),
		PathType::Nvram => ((|x| x.nvram.as_slice()), PathsStore::Single(|x| &mut x.nvram)),
		PathType::Cheats => ((|x| x.cheats.as_slice()), PathsStore::Single(|x| &mut x.cheats)),
		PathType::Snapshots => ((|x| &x.snapshots), PathsStore::Multiple(|x| &mut x.snapshots)),
		PathType::History => (
			(|x| x.history_file.as_slice()),
			PathsStore::Single(|x| &mut x.history_file),
		),
	}
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrefsVideo {
	pub prescale: u8,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub extra_mame_arguments: SmolStr,
}

impl Default for PrefsVideo {
	fn default() -> Self {
		Self {
			prescale: 1,
			extra_mame_arguments: "".into(),
		}
	}
}

impl TryFrom<&crate::ui::VideoSettings> for PrefsVideo {
	type Error = anyhow::Error;

	fn try_from(value: &crate::ui::VideoSettings) -> std::result::Result<Self, Self::Error> {
		let prescale = value.prescale.try_into()?;
		let extra_mame_arguments = value.extra_mame_arguments.as_str().trim().into();
		Ok(Self {
			prescale,
			extra_mame_arguments,
		})
	}
}

impl From<&PrefsVideo> for crate::ui::VideoSettings {
	fn from(value: &PrefsVideo) -> Self {
		let prescale = value.prescale.into();
		let extra_mame_arguments = value.extra_mame_arguments.to_shared_string();
		Self {
			prescale,
			extra_mame_arguments,
		}
	}
}

#[derive(Copy, Clone, Debug, strum::Display, EnumIter, EnumString, EnumProperty, PartialEq, Eq)]
pub enum PreflightProblem {
	#[strum(to_string = "No MAME executable path specified", props(ProblemType = "MAME Executable"))]
	NoMameExecutablePath,
	#[strum(to_string = "No MAME executable found", props(ProblemType = "MAME Executable"))]
	NoMameExecutable,
	#[strum(to_string = "MAME executable file is not executable", props(ProblemType = "MAME Executable"))]
	MameExecutableIsNotExecutable,
	#[strum(to_string = "No valid plugins paths specified", props(ProblemType = "Plugins"))]
	NoPluginsPaths,
	#[strum(to_string = "MAME boot.lua not found", props(ProblemType = "Plugins"))]
	PluginsBootNotFound,
	#[strum(to_string = "BletchMAME worker_ui plugin not found", props(ProblemType = "Plugins"))]
	WorkerUiPluginNotFound,
}

impl PreflightProblem {
	pub fn problem_type(&self) -> Option<PathType> {
		let s = self.get_str("ProblemType")?;
		Some(PathType::from_str(s).unwrap())
	}
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
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

	pub fn description(&self, info_db: Option<&InfoDb>) -> Cow<'_, str> {
		match self {
			PrefsCollection::Builtin(x) => format!("{x}").into(),
			PrefsCollection::MachineSoftware { machine_name } => {
				let machines = info_db.as_ref().map(|db| db.machines()).unwrap_or_default();
				let machine_desc = machines
					.find(machine_name.as_str())
					.map(|x| x.description())
					.unwrap_or(machine_name.as_str());
				format!("Software for \"{machine_desc}\"").into()
			}
			PrefsCollection::Folder { name, items: _ } => Cow::Borrowed(name),
		}
	}
}

#[derive(EnumIter, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, strum::Display, EnumString)]
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
#[serde(rename_all = "camelCase")]
pub struct PrefsItem {
	#[serde(default, flatten, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub video: Option<PrefsVideo>,

	#[serde(flatten)]
	pub details: PrefsItemDetails,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PrefsItemDetails {
	Machine(PrefsMachineItem),
	Software(PrefsSoftwareItem),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrefsMachineItem {
	#[serde(rename = "machine")]
	pub machine_name: String,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default", with = "serde_slots")]
	pub slots: Vec<(String, Option<String>)>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub images: HashMap<String, ImageDesc>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub ram_size: Option<u64>,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub bios: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrefsSoftwareItem {
	#[serde(rename = "softwareList")]
	pub software_list: String,

	pub software: String,

	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub preferred_machines: Option<Vec<String>>,
}

const PREFS: Option<&str> = Some("BletchMAME.json");
const PREFS_BACKUP: Option<&str> = Some("BletchMAME.backup.json");

impl Preferences {
	pub fn load(prefs_path: &Path) -> Result<Option<Self>> {
		// try to load the preferences
		let path = prefs_filename(prefs_path, PREFS)?;
		let result = load_prefs(&path);
		info!("result" = ?result.as_ref().map(|_| ()), "Preferences::load()");

		// did we error?
		if result.is_err() {
			// we did; back up this file
			if let Ok(renamed) = prefs_filename(prefs_path, PREFS_BACKUP) {
				let rc = rename(&path, &renamed);
				info!(path=?path, renamed=?renamed, rc=?rc, "Preferences::load()");
			}
		}

		result
	}

	#[cfg(test)]
	pub fn load_reader(reader: impl Read) -> Result<Self> {
		load_prefs_from_reader(reader)
	}

	pub fn save(&self, prefs_path: &Path) -> Result<()> {
		ensure_directory(&prefs_path);
		let path = prefs_filename(prefs_path, PREFS)?;
		save_prefs(self, &path)
	}

	pub fn save_backup(&self, prefs_path: &Path) -> Result<()> {
		ensure_directory(&prefs_path);
		let path = prefs_filename(prefs_path, PREFS_BACKUP)?;
		save_prefs(self, &path)
	}

	pub fn fresh(prefs_path: Option<SmolStr>) -> Self {
		let json = include_str!("prefs_fresh.json");
		let mut result = load_prefs_from_reader(json.as_bytes()).unwrap();
		let result_paths = Rc::get_mut(&mut result.paths).unwrap();
		result_paths.cfg = prefs_path.clone();
		result_paths.diff = prefs_path.clone();
		result_paths.inis = prefs_path.iter().cloned().collect();
		result_paths.nvram = prefs_path;
		result
	}
}

pub fn prefs_filename(prefs_path: impl AsRef<Path>, filename: Option<&str>) -> Result<PathBuf> {
	let mut pathbuf = prefs_path.as_ref().to_path_buf();
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
			};
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
	let _ = create_dir_all(path);
}

#[cfg(test)]
mod test {
	use std::fs::File;

	use assert_json_diff::assert_json_eq;
	use assert_matches::assert_matches;
	use serde_json::Value;
	use tempdir::TempDir;
	use test_case::test_case;

	use super::Preferences;
	use super::load_prefs_from_reader;
	use super::save_prefs_to_string;

	#[test]
	pub fn fresh_is_indeed_fresh() {
		let prefs = Preferences::fresh(None);
		let json = save_prefs_to_string(&prefs).expect("Failed to save fresh prefs");

		let new_prefs = load_prefs_from_reader(json.as_bytes()).expect("Failed to load saved fresh prefs");
		assert_eq!(prefs, new_prefs);
	}

	#[test_case(0, include_str!("prefs_fresh.json"))]
	#[test_case(1, include_str!("test_data/prefs01.json"))]
	pub fn reserialization(_index: usize, json: &str) {
		let prefs = load_prefs_from_reader(json.as_bytes()).expect("Failed to load prefs");
		let reserialized_json = save_prefs_to_string(&prefs).expect("Failed to save prefs");

		// reeserialize the JSON and compare
		let json_value = serde_json::from_str::<Value>(json).unwrap();
		let reserialized_json_value = serde_json::from_str::<Value>(&reserialized_json).unwrap();
		assert_json_eq!(reserialized_json_value, json_value);

		// reload the prefs and compare
		let reserialized_prefs =
			load_prefs_from_reader(reserialized_json.as_bytes()).expect("Failed to load reserialized prefs");
		assert_eq!(prefs, reserialized_prefs);
	}

	#[test_case(0, &["foo"])]
	#[test_case(1, &["foo", "bar"])]
	pub fn ensure_directory(_index: usize, path_parts: &[&str]) {
		let tmp_dir = TempDir::new("temp").unwrap();
		let mut path = tmp_dir.path().to_path_buf();
		for x in path_parts {
			path = path.join(x);
		}

		// try to ensure the directory
		super::ensure_directory(&path);

		// try to create a file in that dir
		let result = File::create(path.join("file_in_ensured_dir.txt")).map(|_| ());
		assert_matches!(result, Ok(_));
	}
}
