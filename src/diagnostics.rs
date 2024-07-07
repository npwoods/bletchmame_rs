use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::info::InfoDb;

pub fn info_db_from_xml_file(path: impl AsRef<Path>) {
	let file = File::open(path).unwrap();
	let mut reader = BufReader::new(file);
	let _ = InfoDb::from_listxml_output(&mut reader, |_| false).unwrap().unwrap();
	println!("Success");
}
