use std::fmt::Display;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use derive_enum_all_values::AllValues;
use dirs::config_dir;
use itertools::Itertools;
use itertools::Position;
use serde::Deserialize;
use serde::Serialize;
use slint::LogicalSize;

use crate::Error;
use crate::Result;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub paths: PrefsPaths,
	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub window_size: Option<PrefsSize>,
	#[serde(default)]
	pub collections: Vec<PrefsCollectionItem>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrefsPaths {
	pub mame_executable: Option<String>,
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
#[serde(rename_all = "camelCase")]
pub struct PrefsCollectionItem {
	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub selected: PrefsSelection,

	#[serde(flatten)]
	pub inner: InnerCollectionItem,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PrefsSelection {
	Bool(bool),
	String(String),
}

impl Default for PrefsSelection {
	fn default() -> Self {
		PrefsSelection::Bool(false)
	}
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum InnerCollectionItem {
	Builtin(BuiltinCollectionItem),
	Machines(MachinesCollectionItem),
	Software(SoftwareCollectionItem),
	Folder(FolderCollectionItem),
}

#[derive(AllValues, Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", tag = "subtype")]
pub enum BuiltinCollectionItem {
	All,
	Source,
	Year,
	Manufacturer,
	Cpu,
	Sound,
}

impl Display for BuiltinCollectionItem {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let s = match self {
			BuiltinCollectionItem::All => "All Systems",
			BuiltinCollectionItem::Source => "Source",
			BuiltinCollectionItem::Year => "Year",
			BuiltinCollectionItem::Manufacturer => "Manufacturer",
			BuiltinCollectionItem::Cpu => "CPU",
			BuiltinCollectionItem::Sound => "Sound",
		};
		write!(f, "{s}")
	}
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MachinesCollectionItem {
	#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
	pub name: Option<String>,
	pub machines: Vec<String>,
	#[serde(default)]
	pub show_software: bool,
	#[serde(default)]
	pub custom_software_directories: Vec<CustomSoftwareDirectory>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomSoftwareDirectory {
	pub path: String,
	#[serde(default)]
	pub recursive: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SoftwareCollectionItem {
	#[serde(default)]
	pub name: Option<String>,
	pub machine: String,
	pub software: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderCollectionItem {
	pub name: String,
	pub children: Vec<PrefsCollectionItem>,
}

impl PrefsCollectionItem {
	pub fn walk<'a>(
		items: &'a [PrefsCollectionItem],
		mut callback: impl FnMut(&'a PrefsCollectionItem, &[usize], Position),
	) {
		let mut path = Vec::new();
		Self::walk_internal(items, &mut callback, &mut path);
	}

	fn walk_internal<'a>(
		items: &'a [PrefsCollectionItem],
		callback: &mut impl FnMut(&'a PrefsCollectionItem, &[usize], Position),
		path: &mut Vec<usize>,
	) {
		for (position, (index, item)) in items.iter().enumerate().with_position() {
			path.push(index);
			callback(item, &path, position);
			if let InnerCollectionItem::Folder(x) = &item.inner {
				Self::walk_internal(&x.children, callback, path);
			}
			path.truncate(path.len() - 1);
		}
	}

	pub fn process(
		items: Vec<PrefsCollectionItem>,
		mut callback: impl FnMut(Vec<PrefsCollectionItem>) -> Vec<PrefsCollectionItem>,
	) -> Vec<PrefsCollectionItem> {
		Self::internal_process(items, &mut callback)
	}

	fn internal_process(
		items: Vec<PrefsCollectionItem>,
		callback: &mut impl FnMut(Vec<PrefsCollectionItem>) -> Vec<PrefsCollectionItem>,
	) -> Vec<PrefsCollectionItem> {
		let new_items = items
			.into_iter()
			.map(|item| PrefsCollectionItem {
				selected: item.selected,
				inner: if let InnerCollectionItem::Folder(x) = item.inner {
					InnerCollectionItem::Folder(FolderCollectionItem {
						name: x.name,
						children: Self::internal_process(x.children, callback),
					})
				} else {
					item.inner
				},
			})
			.collect();
		callback(new_items)
	}
}

const PREFS: Option<&str> = Some("BletchMAME.json");

impl Preferences {
	pub fn load() -> Result<Self> {
		let path = prefs_filename(PREFS).map_err(prefs_load_error)?;
		load_prefs(&path)
	}

	pub fn save(&self) -> Result<()> {
		let path = prefs_filename(PREFS).map_err(prefs_save_error)?;
		save_prefs(self, &path)
	}

	pub fn fresh() -> Self {
		let json = include_str!("prefs_fresh.json");
		load_prefs_from_reader(json.as_bytes()).unwrap()
	}

	pub fn move_collection(&mut self, path: &[usize], delta: Option<i8>) {
		move_within_tree(&mut self.collections, path, delta, |item| {
			let InnerCollectionItem::Folder(item) = &mut item.inner else {
				panic!("Invalid path");
			};
			&mut item.children
		});
	}
}

pub fn prefs_filename(filename: Option<&str>) -> Result<PathBuf> {
	let mut pathbuf = config_dir().ok_or(Error::CantFindPreferencesDirectory)?;
	pathbuf.push("BletchMAME");
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
	let reader = BufReader::new(reader);
	let prefs = serde_json::from_reader(reader).map_err(prefs_load_error)?;
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

fn move_within_tree<T>(
	mut tree: &mut Vec<T>,
	path: &[usize],
	delta: Option<impl Into<isize>>,
	traverse: impl Fn(&mut T) -> &mut Vec<T>,
) {
	// change the path to not have the final element
	let remove_index = *path.last().unwrap();
	let reinsert_index = delta.map(|x| remove_index.checked_add_signed(x.into()).unwrap());
	let path = &path[..(path.len() - 1)];

	// traverse the tree
	for &index in path {
		let element = &mut tree[index];
		tree = traverse(element);
	}

	// and manipulate the final item
	let element = tree.remove(remove_index);
	if let Some(reinsert_index) = reinsert_index {
		tree.insert(reinsert_index, element);
	}
}

#[cfg(test)]
mod test {
	use super::load_prefs_from_reader;
	use super::save_prefs_to_string;
	use super::Preferences;

	#[test]
	pub fn test() {
		let prefs = Preferences::fresh();
		let json = save_prefs_to_string(&prefs).expect("Failed to save fresh prefs");

		let fresh_json = include_str!("prefs_fresh.json");
		assert_eq!(fresh_json.replace("\r", ""), json.replace("\r", ""));

		let new_prefs = load_prefs_from_reader(json.as_bytes()).expect("Failed to load saved fresh prefs");
		assert_eq!(prefs, new_prefs);
	}
}
