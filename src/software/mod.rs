use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use process::process_xml;

use crate::Error;
use crate::Result;

mod process;

pub struct SoftwareList {
	pub name: Rc<str>,
	pub description: Rc<str>,
	pub software: Vec<Rc<Software>>,
}

#[derive(Debug)]
pub struct Software {
	pub name: Rc<str>,
	pub description: Rc<str>,
	pub year: Rc<str>,
	pub publisher: Rc<str>,
}

impl SoftwareList {
	pub fn load(path: impl AsRef<Path>) -> Result<Self> {
		let file = File::open(path).map_err(|x| Error::SoftwareListLoad(x.into()))?;
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
			.finish()
	}
}

pub struct SoftwareListDispenser<'a> {
	software_list_paths: &'a [String],
	map: HashMap<String, Result<Rc<SoftwareList>>>,
}

impl<'a> SoftwareListDispenser<'a> {
	pub fn new(software_list_paths: &'a [String]) -> Self {
		Self {
			software_list_paths,
			map: HashMap::new(),
		}
	}

	pub fn get(&mut self, software_list_name: &str) -> Option<Rc<SoftwareList>> {
		let software_list_name = software_list_name.to_string();
		self.map
			.entry(software_list_name)
			.or_insert_with_key(|name| load_software_list(self.software_list_paths, name))
			.as_ref()
			.ok()
			.cloned()
	}
}

fn load_software_list(paths: &[String], name: &str) -> Result<Rc<SoftwareList>> {
	let mut err = Error::SoftwareListLoadNoPaths.into();
	paths
		.iter()
		.filter(|&path| !path.is_empty())
		.filter_map(|path| {
			let mut path = PathBuf::from(path);
			path.push(name);
			path.set_extension("xml");
			match SoftwareList::load(&path) {
				Ok(x) => Some(Rc::new(x)),
				Err(e) => {
					err = e;
					None
				}
			}
		})
		.next()
		.ok_or(err)
}
