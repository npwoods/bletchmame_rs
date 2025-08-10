use std::cell::Cell;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::path::Path;

use anyhow::Error;
use anyhow::Result;
use path_absolutize::Absolutize;
use strum::EnumString;
use strum::VariantArray;

use crate::prefs::Preferences;
use crate::prefs::PrefsPaths;
use crate::prefs::pathtype::PathType;

pub struct ImportMameIni(Vec<(ImportMameIniOption, Cell<Disposition>)>);

#[derive(Debug, PartialEq)]
pub struct ImportMameIniOption {
	pub path_type: PathType,
	pub value: String,
}

#[derive(Copy, Clone, Debug, PartialEq, EnumString, strum::Display)]
pub enum Disposition {
	#[strum(to_string = "Ignore")]
	Ignore,
	#[strum(to_string = "Supplement")]
	Supplement,
	#[strum(to_string = "Replace")]
	Replace,
	#[strum(to_string = "(already present)")]
	AlreadyPresent,
}

impl ImportMameIni {
	pub fn read_mame_ini(path: impl AsRef<Path>, prefs_paths: &PrefsPaths) -> Result<Self> {
		let path = path.as_ref();

		let parent = path.parent().unwrap_or(Path::new("."));
		let absolutize_path = |path: &str| {
			Path::new(path)
				.absolutize_from(parent)
				.ok()
				.and_then(|p| p.into_owned().into_os_string().into_string().ok())
				.unwrap_or_else(|| path.to_string())
		};

		let file = File::open(path)?;
		let reader = BufReader::new(file);
		let entries = read_mame_ini(reader, absolutize_path)?
			.into_iter()
			.map(|opt| {
				let disposition = default_disposition(&opt, prefs_paths);
				let disposition = Cell::new(disposition);
				(opt, disposition)
			})
			.collect::<Vec<_>>();
		Ok(Self(entries))
	}

	pub fn entries(&self) -> &'_ [(ImportMameIniOption, Cell<Disposition>)] {
		&self.0
	}

	pub fn apply(&self, _prefs: &mut Preferences) {
		todo!()
	}
}

fn read_mame_ini(reader: impl BufRead, absolutize_path: impl Fn(&str) -> String) -> Result<Vec<ImportMameIniOption>> {
	let arg_map = PathType::VARIANTS
		.iter()
		.filter_map(|&path_type| {
			path_type.mame_argument().map(|mame_argument| {
				let mame_argument = mame_argument.trim_start_matches('-');
				(mame_argument, path_type)
			})
		})
		.collect::<HashMap<_, _>>();

	// read through the file
	reader
		.lines()
		.flat_map(|line| match line {
			Ok(line) => {
				if let Some((name, value)) = parse_mame_ini_line(&line) {
					arg_map
						.get(name)
						.map(|&path_type| {
							value
								.split(';')
								.map(str::trim)
								.filter(|value| !value.is_empty())
								.map(|value| {
									let value = absolutize_path(value);
									let ini_option = ImportMameIniOption { path_type, value };
									Ok(ini_option)
								})
								.collect::<Vec<_>>()
						})
						.unwrap_or_default()
				} else {
					[].into()
				}
			}
			Err(e) => [Err(Error::from(e))].into(),
		})
		.collect::<Result<Vec<_>>>()
}

fn parse_mame_ini_line(line: &str) -> Option<(&'_ str, &'_ str)> {
	// so-called "MAME INI" files are not conventional INIs, so we need this
	let (line, _) = line.split_once('#').unwrap_or((line, ""));
	let (name, value) = line.split_once(' ')?;
	let name = trim_and_strip_quotes(name);
	let value = trim_and_strip_quotes(value);
	(!name.is_empty()).then_some((name, value))
}

fn trim_and_strip_quotes(s: &str) -> &'_ str {
	let s = s.trim();
	s.strip_prefix('\"').and_then(|s| s.strip_suffix('\"')).unwrap_or(s)
}

fn default_disposition(opt: &ImportMameIniOption, prefs_paths: &PrefsPaths) -> Disposition {
	let opt_path = Path::new(&opt.value);

	let mut path_iter = prefs_paths
		.by_type(opt.path_type)
		.iter()
		.filter_map(|path| Path::new(path).absolutize().ok());
	if path_iter.any(|this_path| this_path.as_ref() == opt_path) {
		Disposition::AlreadyPresent
	} else if opt.path_type.is_multi() {
		Disposition::Supplement
	} else {
		Disposition::Replace
	}
}

#[cfg(test)]
mod test {
	use std::io::BufReader;
	use std::io::Cursor;

	use test_case::test_case;

	use crate::importmameini::ImportMameIniOption;
	use crate::prefs::pathtype::PathType;

	use super::read_mame_ini;

	#[test]
	fn general() {
		let ini_str = r#"
			rompath "/foo/bar/roms;/foo/baz"
			samplepath /foo/bar/samples
		"#;

		let expected = [
			(PathType::Roms, "/foo/bar/roms"),
			(PathType::Roms, "/foo/baz"),
			(PathType::Samples, "/foo/bar/samples"),
		];
		let expected = expected
			.into_iter()
			.map(|(path_type, value)| {
				let value = value.to_string();
				ImportMameIniOption { path_type, value }
			})
			.collect::<Vec<_>>();

		let cursor = Cursor::new(ini_str);
		let reader = BufReader::new(cursor);
		let actual = read_mame_ini(reader, |x| x.to_string()).unwrap();
		assert_eq!(expected, actual);
	}

	#[test_case(0, "", None)]
	#[test_case(1, "   ", None)]
	#[test_case(2, "# COMMENT", None)]
	#[test_case(3, "alpha bravo", Some(("alpha", "bravo")))]
	#[test_case(4, "alpha bravo # COMMENT", Some(("alpha", "bravo")))]
	#[test_case(5, "alpha \"bravo\" # COMMENT", Some(("alpha", "bravo")))]
	#[test_case(6, "alpha \"bravo charlie\" # COMMENT", Some(("alpha", "bravo charlie")))]
	fn parse_mame_ini_line(_index: usize, line: &str, expected: Option<(&str, &str)>) {
		let actual = super::parse_mame_ini_line(line);
		assert_eq!(expected, actual);
	}
}
