use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt::Debug;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::scope;

use anyhow::Error;
use anyhow::Result;
use smol_str::SmolStr;

use crate::info;
use crate::info::InfoDb;
use crate::info::View;
use crate::software::process::process_xml;

mod process;

pub struct SoftwareList {
	pub name: SmolStr,
	pub description: SmolStr,
	pub software: Vec<Arc<Software>>,
}

#[derive(Debug)]
pub struct Software {
	pub name: SmolStr,
	pub description: SmolStr,
	pub year: SmolStr,
	pub publisher: SmolStr,
	pub parts: Vec<SoftwarePart>,
}

#[derive(Debug)]
pub struct SoftwarePart {
	#[allow(dead_code)]
	pub name: SmolStr,

	pub interface: SmolStr,
}

impl SoftwareList {
	pub fn load(path: impl AsRef<Path>) -> Result<Self> {
		let file = File::open(path)?;
		let file = BufReader::new(file);
		Self::from_reader(file)
	}

	pub fn from_reader(reader: impl BufRead) -> Result<Self> {
		process_xml(reader)
	}
}

impl Debug for SoftwareList {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("SoftwareList")
			.field("name", &self.name)
			.field("description", &self.description)
			.field("software.len()", &self.software.len())
			.finish_non_exhaustive()
	}
}

pub struct SoftwareListDispenser<'a> {
	pub info_db: &'a InfoDb,
	software_list_paths: &'a [SmolStr],
	map: HashMap<String, (info::SoftwareList<'a>, Arc<SoftwareList>)>,
}

impl<'a> SoftwareListDispenser<'a> {
	pub fn new(info_db: &'a InfoDb, software_list_paths: &'a [SmolStr]) -> Self {
		Self {
			info_db,
			software_list_paths,
			map: HashMap::new(),
		}
	}

	pub fn get(&mut self, software_list_name: &str) -> Result<(info::SoftwareList<'a>, Arc<SoftwareList>)> {
		let entry = self.map.entry(software_list_name.to_string());
		let (info_db_software_list, software_list) = match entry {
			Entry::Occupied(entry) => entry.get().clone(),
			Entry::Vacant(entry) => {
				let info_db_software_list = self.info_db.software_lists().find(software_list_name)?;
				let software_list = load_software_list(self.software_list_paths, software_list_name)?;
				entry.insert((info_db_software_list, software_list.clone()));
				(info_db_software_list, software_list)
			}
		};
		Ok((info_db_software_list, software_list))
	}

	pub fn get_all(&mut self) -> Vec<(info::SoftwareList<'a>, Arc<SoftwareList>)> {
		scope(|scope| {
			let info_db = self.info_db;
			let paths = self.software_list_paths;
			let threads = info_db
				.software_lists()
				.iter()
				.map(|info_db_software_list| {
					scope.spawn(move || {
						load_software_list(paths, info_db_software_list.name())
							.map(|software_list| (info_db_software_list, software_list))
					})
				})
				.collect::<Vec<_>>();

			threads
				.into_iter()
				.filter_map(|handle| handle.join().unwrap().ok())
				.collect::<Vec<_>>()
		})
	}

	pub fn is_empty(&self) -> bool {
		self.map.is_empty()
	}
}

fn load_software_list(paths: &[impl AsRef<str>], name: &str) -> Result<Arc<SoftwareList>> {
	let mut err = Error::msg("Error loading software list: No paths specified");
	paths
		.iter()
		.map(|x| x.as_ref())
		.filter(|&path| !path.is_empty())
		.filter_map(|path| {
			let mut path = PathBuf::from(path);
			path.push(name);
			path.set_extension("xml");
			match SoftwareList::load(&path) {
				Ok(x) => Some(x.into()),
				Err(e) => {
					err = e;
					None
				}
			}
		})
		.next()
		.ok_or(err)
}

pub fn is_valid_software_list_name(s: &str) -> bool {
	!s.is_empty()
		&& s.chars()
			.all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == '_')
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	#[test_case(0, "", false)]
	#[test_case(1, "abcde", true)]
	#[test_case(2, "Abcde", false)]
	#[test_case(3, "ABCDE", false)]
	#[test_case(4, "abcde_fghij", true)]
	#[test_case(5, "abcde fghij", false)]
	#[test_case(6, "foo.img", false)]
	#[test_case(7, "/foo/bar.img", false)]
	#[test_case(8, "C:\\foo\\bar.img", false)]
	fn is_valid_software_list_name(_index: usize, s: &str, expected: bool) {
		let actual = super::is_valid_software_list_name(s);
		assert_eq!(expected, actual);
	}
}
