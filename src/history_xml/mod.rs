mod parse;

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use anyhow::Result;
use slint::SharedString;
use smol_str::SmolStr;

use crate::history_xml::parse::parse_from_reader;

#[derive(Debug, Default)]
pub struct HistoryXml {
	systems: HashMap<SmolStr, SharedString>,
	software: HashMap<(SmolStr, SmolStr), SharedString>,
}

impl HistoryXml {
	pub fn load(path: impl AsRef<Path>) -> Result<Self> {
		let file = File::open(path.as_ref())?;
		let reader = BufReader::new(file);
		parse_from_reader(reader)
	}

	pub fn by_system(&self, system: impl AsRef<str>) -> Option<SharedString> {
		self.systems.get(system.as_ref()).cloned()
	}

	pub fn by_software(&self, list: impl Into<SmolStr>, name: impl Into<SmolStr>) -> Option<SharedString> {
		self.software.get(&(list.into(), name.into())).cloned()
	}
}

#[cfg(test)]
mod tests {
	use test_case::test_case;

	use super::parse_from_reader;

	#[test_case(0, include_str!("test_data/history.xml"), "litware64", Some("The \"Litware 64\" and \"Litware 128\" never existed"))]
	#[test_case(1, include_str!("test_data/history.xml"), "litware128", Some("The \"Litware 64\" and \"Litware 128\" never existed"))]
	#[test_case(2, include_str!("test_data/history.xml"), "DOESNT_EXIST", None)]
	fn by_system(_index: usize, xml: &str, system: &str, expected: Option<&str>) {
		let history = parse_from_reader(xml.as_bytes()).unwrap();
		let actual = history.by_system(system);
		let actual = actual.as_deref();
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/history.xml"), "contoso_flop", "contoso_alpha01", Some("Contoso's alpha invaders never existed either"))]
	#[test_case(1, include_str!("test_data/history.xml"), "contoso_flop", "contoso_alpha02", Some("Contoso's alpha invaders never existed either"))]
	#[test_case(2, include_str!("test_data/history.xml"), "contoso_flop", "DOESNT_EXIST", None)]
	fn by_software(_index: usize, xml: &str, list: &str, name: &str, expected: Option<&str>) {
		let history = parse_from_reader(xml.as_bytes()).unwrap();
		let actual = history.by_software(list, name);
		let actual = actual.as_deref();
		assert_eq!(expected, actual);
	}
}
