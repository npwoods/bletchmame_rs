use std::fmt::Display;
use std::fmt::Formatter;
use std::fs::File;
use std::io::Cursor;
use std::io::Read;
use std::io::Seek;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use default_ext::DefaultExt;
use easy_ext::ext;
use itertools::Either;
use itertools::Itertools;
use sevenz_rust2::ArchiveReader as SevenZArchiveReader;
use slint::SharedString;
use smol_str::SmolStr;
use strum::EnumProperty;
use tracing::debug;
use zip::ZipArchive;
use zip::read::ZipFile;
use zip::result::ZipError;
use zip::result::ZipResult;

use crate::assethash::AssetHash;
use crate::chd::chd_asset_hash;
use crate::info::AssetStatus;
use crate::info::Machine;
use crate::info::View;
use crate::mconfig::MachineConfig;

#[derive(Clone, Debug)] // TODO - `Clone` should not be necessary
pub struct Asset {
	pub kind: AssetKind,
	pub name: SharedString,
	pub size: Option<u64>,
	machine_names: Arc<[SmolStr]>,
	asset_hash: AssetHash,
	status: AssetStatus,
	is_optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetKind {
	Rom,
	Disk,
	Sample,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditSeverity {
	Info,
	Warning,
	Fail,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)] // TODO - `Clone` should not be necessary
pub struct AuditResult {
	pub path: Option<(PathBuf, PathType)>,
	pub messages: Box<[AuditMessage]>,
}

#[derive(Clone, Debug, PartialEq, Eq)] // TODO - `Clone` should not be necessary
pub enum AuditMessage {
	NotFound,
	NotFoundNoGoodDump,
	NotFoundButOptional,
	WrongLength { expected: u64, found: u64 },
	WrongChecksums { expected: AssetHash, found: AssetHash },
	NeedsRedump,
	NoGoodDump,
}

/// Used for diagnostic purposes
#[derive(Copy, Clone, Debug)]
#[allow(dead_code)]
enum MachineType<'a> {
	Root,
	Slot,
	DeviceRef(Option<&'a str>),
}

impl Asset {
	pub fn from_machine_config(machine_config: &MachineConfig) -> Vec<Self> {
		debug!(?machine_config, "Asset::from_machine_config()");

		let mut results = Vec::new();
		Self::from_machine_internal(&mut results, machine_config.machine(), None, MachineType::Root);
		machine_config.visit_slots(|_, _, _, _, slot_data| {
			if let Some(machine) = slot_data.map(|(_, machine_config)| machine_config.machine()) {
				Self::from_machine_internal(&mut results, machine, None, MachineType::Slot);
			}
		});

		// remove duplicates and return
		results
			.into_iter()
			.unique_by(|x| (x.kind, x.name.clone(), x.machine_names.clone()))
			.collect()
	}

	fn from_machine_internal(
		results: &mut Vec<Self>,
		machine: Machine<'_>,
		bios: Option<&str>,
		machine_type: MachineType<'_>,
	) {
		// we were passed a BIOS; if `None` was specified use the machine's default BIOS
		let bios = bios.or_else(|| {
			machine
				.default_biosset_index()
				.map(|index| machine.biossets().get(index).unwrap().name())
		});

		debug!(machine=?machine.name(), ?bios, ?machine_type, "Asset::from_machine_internal()");

		let machine_names = [Some(machine.name()), machine.rom_of().map(|x| x.name())]
			.iter()
			.flatten()
			.copied()
			.map(SmolStr::from)
			.collect::<Arc<[_]>>();
		let roms = machine
			.roms()
			.iter()
			.filter(|r| r.bios().is_none_or(|b| bios == Some(b)))
			.map(|rom| Asset {
				kind: AssetKind::Rom,
				name: rom.name().into(),
				size: rom.size().into(),
				machine_names: machine_names.clone(),
				asset_hash: rom.asset_hash(),
				status: rom.status(),
				is_optional: rom.is_optional(),
			});
		let disks = machine.disks().iter().map(|disk| Asset {
			kind: AssetKind::Disk,
			name: format!("{}.chd", disk.name()).into(),
			size: None,
			machine_names: machine_names.clone(),
			asset_hash: disk.asset_hash(),
			status: disk.status(),
			is_optional: disk.is_optional(),
		});
		let samples = machine.samples().iter().map(|sample| Asset {
			kind: AssetKind::Sample,
			name: format!("{}.wav", sample.name()).into(),
			size: None,
			machine_names: machine_names.clone(),
			asset_hash: AssetHash::default(),
			status: AssetStatus::Good,
			is_optional: true, // samples are always optional; MAME just doesn't play the same if they are missing
		});
		results.extend(roms.chain(disks).chain(samples));

		// add devices references
		for device_ref in machine.device_refs().iter() {
			if let Some(machine) = device_ref.machine() {
				let machine_type = MachineType::DeviceRef(device_ref.tag());
				Self::from_machine_internal(results, machine, None, machine_type);
			}
		}
	}

	pub fn run_audit(&self, rom_paths: &[impl AsRef<Path>], sample_paths: &[impl AsRef<Path>]) -> AuditResult {
		// wrap these up in a uniform signature
		type HashFunc = fn(&mut dyn Read) -> Result<AssetHash>;
		fn hash_func_rom(file: &mut dyn Read) -> Result<AssetHash> {
			AssetHash::calculate(file)
		}
		fn hash_func_sample(_file: &mut dyn Read) -> Result<AssetHash> {
			unreachable!("Samples don't get hashed");
		}
		fn hash_func_disk(file: &mut dyn Read) -> Result<AssetHash> {
			chd_asset_hash(file)
		}

		// do different things based on the `AssetKind`
		let (asset_paths, support_archives, hash_func) = match self.kind {
			AssetKind::Rom => (Either::Left(rom_paths), true, hash_func_rom as HashFunc),
			AssetKind::Sample => (Either::Right(sample_paths), true, hash_func_sample as HashFunc),
			AssetKind::Disk => (Either::Left(rom_paths), false, hash_func_disk as HashFunc),
		};

		// normalize the paths iterator
		let paths_iter = asset_paths
			.map_left(|paths| paths.iter().map(|x| x.as_ref()))
			.map_right(|paths| paths.iter().map(|x| x.as_ref()));

		// now do the heavy lifting
		audit_single(
			&self.name,
			self.size,
			&self.asset_hash,
			self.status,
			self.is_optional,
			self.machine_names.as_ref(),
			paths_iter,
			support_archives,
			hash_func,
		)
	}
}

impl AuditResult {
	pub fn severity(&self) -> AuditSeverity {
		self.messages
			.iter()
			.map(|m| m.severity())
			.max()
			.unwrap_or(AuditSeverity::Info)
	}
}

impl AuditMessage {
	pub fn severity(&self) -> AuditSeverity {
		match self {
			Self::NotFoundButOptional | Self::NeedsRedump | Self::NoGoodDump => AuditSeverity::Info,
			Self::WrongLength { .. } | Self::WrongChecksums { .. } => AuditSeverity::Warning,
			Self::NotFound | Self::NotFoundNoGoodDump => AuditSeverity::Fail,
		}
	}
}

impl Display for AuditMessage {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NotFound => write!(f, "NOT FOUND"),
			Self::NotFoundNoGoodDump => write!(f, "NOT FOUND - NO GOOD DUMP KNOWN"),
			Self::NotFoundButOptional => write!(f, "NOT FOUND BUT OPTIONAL"),
			Self::WrongLength { expected, found } => write!(f, "WRONG LENGTH (expected: {expected} found {found})"),
			Self::WrongChecksums { expected, found } => write!(f, "WRONG CHECKSUMS EXPECTED {expected} FOUND {found}"),
			Self::NeedsRedump => write!(f, "NEEDS REDUMP"),
			Self::NoGoodDump => write!(f, "NO GOOD DUMP KNOWN"),
		}
	}
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, EnumProperty)]
pub enum PathType {
	File,
	#[strum(props(ArchiveExtension = "zip"))]
	Zip,
	#[strum(props(ArchiveExtension = "7z"))]
	SevenZ,
}

#[allow(clippy::too_many_arguments)]
fn audit_single<'a>(
	asset_name: &str,
	expected_size: Option<u64>,
	expected_asset_hash: &AssetHash,
	status: AssetStatus,
	is_optional: bool,
	machine_names: &[impl AsRef<str>],
	paths_iter: impl Iterator<Item = &'a Path> + Clone,
	support_archives: bool,
	hash_func: fn(&mut dyn Read) -> Result<AssetHash>,
) -> AuditResult {
	let path_types = if support_archives {
		[PathType::File, PathType::Zip, PathType::SevenZ].as_slice()
	} else {
		[PathType::File].as_slice()
	};

	machine_names
		.iter()
		.flat_map(|machine_name| paths_iter.clone().map(|path| (machine_name.as_ref(), path)))
		.flat_map(|(machine_name, path)| path_types.iter().map(move |path_type| (machine_name, path, *path_type)))
		.filter_map(|(machine_name, path, path_type)| {
			try_audit(
				asset_name,
				expected_size,
				expected_asset_hash,
				status,
				machine_name,
				path,
				path_type,
				hash_func,
			)
			.ok()
		})
		.next()
		.unwrap_or_else(|| {
			let message = match (status, is_optional) {
				(AssetStatus::NoDump, _) => AuditMessage::NotFoundNoGoodDump,
				(_, false) => AuditMessage::NotFound,
				(_, true) => AuditMessage::NotFoundButOptional,
			};
			let messages = [message].into();
			AuditResult { path: None, messages }
		})
}

#[allow(clippy::too_many_arguments)]
fn try_audit(
	asset_name: &str,
	expected_size: Option<u64>,
	expected_asset_hash: &AssetHash,
	status: AssetStatus,
	machine_name: &str,
	path: &Path,
	path_type: PathType,
	hash_func: fn(&mut dyn Read) -> Result<AssetHash>,
) -> Result<AuditResult> {
	// open the file
	let mut path = path.join(machine_name);
	if let Some(archive_extension) = path_type.get_str("ArchiveExtension") {
		path.set_extension(archive_extension);
	} else {
		path = path.join(asset_name);
	}
	let file_result = File::open(&path);
	debug!(?path_type, ?path, ?file_result, "try_audit(): Invoked File::open()");
	let mut file = file_result?;

	// these operations depend on the type of file
	let (actual_size, actual_hash) = match path_type {
		PathType::File => {
			let actual_size = file.metadata()?.len();
			let actual_hash = (!expected_asset_hash.is_default())
				.then(|| hash_func(&mut file))
				.transpose()?;
			(actual_size, actual_hash)
		}
		PathType::Zip => {
			let mut zip_archive = ZipArchive::new(file)?;
			let mut zip_file = zip_archive.by_name_or_crc(asset_name, expected_asset_hash.crc)?;
			let actual_size = zip_file.size();
			let actual_hash = (!expected_asset_hash.is_default())
				.then(|| hash_func(&mut zip_file))
				.transpose()?;
			(actual_size, actual_hash)
		}
		PathType::SevenZ => {
			let data = SevenZArchiveReader::new(file, Default::default())?
				.read_file_by_name_or_crc(asset_name, expected_asset_hash.crc)?;
			let actual_size = data.len() as u64;
			let actual_hash = (!expected_asset_hash.is_default())
				.then(|| hash_func(&mut Cursor::new(&data)))
				.transpose()?;
			(actual_size, actual_hash)
		}
	};

	let mut messages = Vec::new();

	if let Some(expected) = expected_size
		&& expected != actual_size
	{
		let msg = AuditMessage::WrongLength {
			expected,
			found: actual_size,
		};
		messages.push(msg);
	}

	if let Some(actual_hash) = actual_hash
		&& !actual_hash.matches(expected_asset_hash)
	{
		let msg = AuditMessage::WrongChecksums {
			expected: *expected_asset_hash,
			found: actual_hash,
		};
		messages.push(msg);
	}

	match status {
		AssetStatus::Good => {}
		AssetStatus::BadDump => messages.push(AuditMessage::NeedsRedump),
		AssetStatus::NoDump => messages.push(AuditMessage::NoGoodDump),
	}

	let path = Some((path, path_type));
	let messages = messages.into();
	Ok(AuditResult { path, messages })
}

#[ext(ZipArchiveExt)]
impl<R> ZipArchive<R>
where
	R: Read + Seek,
{
	/// This is an emulation of MAME's behavior by which you can load assets by CRC even if the name is wrong
	pub fn by_name_or_crc(&mut self, name: &str, crc: Option<u32>) -> ZipResult<ZipFile<'_, R>> {
		let file_number = match (self.index_for_name(name), crc) {
			(Some(index), _) => Some(index),
			(None, None) => None,
			(None, Some(crc)) => {
				(0..self.len()).find(|&file_number| self.by_index(file_number).is_ok_and(|file| file.crc32() == crc))
			}
		};
		file_number
			.map(|file_number| self.by_index(file_number))
			.unwrap_or(Err(ZipError::FileNotFound))
	}
}

#[ext(SevenZArchiveReaderExt)]
impl<R> SevenZArchiveReader<R>
where
	R: Read + Seek,
{
	/// Same deal as ZipArchive::by_name_or_crc
	pub fn read_file_by_name_or_crc(
		&mut self,
		name: &str,
		crc: Option<u32>,
	) -> std::result::Result<Vec<u8>, sevenz_rust2::Error> {
		match self.read_file(name) {
			Ok(data) => Ok(data),
			Err(e) => {
				let by_crc = crc.and_then(|crc| {
					self.archive()
						.files
						.iter()
						.find(|f| f.has_stream && f.has_crc && f.crc == crc as u64)
						.map(|f| f.name.clone())
						.and_then(|entry_name| self.read_file(&entry_name).ok())
				});
				by_crc.ok_or(e)
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use std::io::Cursor;
	use std::io::Read;
	use std::ops::ControlFlow;

	use sevenz_rust2::ArchiveReader as SevenZArchiveReader;
	use test_case::test_case;
	use zip::ZipArchive;

	use crate::info::InfoDb;
	use crate::mconfig::MachineConfig;

	use super::Asset;
	use super::SevenZArchiveReaderExt as _;
	use super::ZipArchiveExt as _;

	#[test_case(0, include_str!("../info/test_data/listxml_alienar.xml"), "alienar")]
	#[test_case(1, include_str!("../info/test_data/listxml_coco.xml"), "coco2b")]
	#[test_case(2, include_str!("../info/test_data/listxml_c64.xml"), "c64")]
	#[test_case(3, include_str!("../info/test_data/listxml_fake.xml"), "fake")]
	#[test_case(4, include_str!("../info/test_data/listxml_fake.xml"), "blah")]
	#[test_case(5, include_str!("../info/test_data/listxml_fake.xml"), "fakefake")]
	fn assets_from_machine_config(_index: usize, xml: &str, machine_name: &str) {
		// set the insta snapshot suffix; this is a parameterized test
		let mut settings = insta::Settings::clone_current();
		settings.set_snapshot_suffix(machine_name);
		let _guard = settings.bind_to_scope();

		// load the InfoDb
		let info_db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap()
			.into();

		// create a MachineConifg
		let opts: &[(&str, Option<&str>)] = &[];
		let machine_config = MachineConfig::from_machine_name_and_slots(info_db, machine_name, opts).unwrap();

		// identify audit assets
		let assets = Asset::from_machine_config(&machine_config);

		// and validate
		insta::assert_debug_snapshot!(assets);
	}

	#[test_case(0, "correct_name.bin", None, Ok(("correct_name.bin", 0x3b5b2bc1)))]
	#[test_case(1, "nonexistent_name.bin", Some(0x098825db), Ok(("wrong_name.bin", 0x098825db)))]
	#[test_case(2, "correct_name.bin", Some(0x098825db), Ok(("correct_name.bin", 0x3b5b2bc1)))]
	#[test_case(3, "nonexistent.bin", None, Err(()))]
	#[test_case(4, "nonexistent.bin", Some(0xdeadbeef), Err(()))]
	fn by_name_or_crc(_index: usize, name: &str, crc: Option<u32>, expected: Result<(&str, u32), ()>) {
		// access the ZIP version
		let zip_bytes = include_bytes!("test_data/ziparchive01.zip");
		let cursor = Cursor::new(zip_bytes);
		let mut archive = ZipArchive::new(cursor).unwrap();

		// and validate
		let mut file_result = archive.by_name_or_crc(name, crc);
		let actual = file_result
			.as_ref()
			.map(|file| (file.name(), file.crc32()))
			.map_err(|_| ());
		assert_eq!(expected, actual);

		// get the bytes to compare it with 7z
		let expected = file_result
			.as_mut()
			.map(|file| {
				let mut vec = Vec::new();
				file.read_to_end(&mut vec).unwrap();
				vec
			})
			.map_err(|_| ());

		// access the 7z version
		let sevenz_bytes = include_bytes!("test_data/7zarchive01.7z");
		let cursor = Cursor::new(sevenz_bytes);
		let mut archive = SevenZArchiveReader::new(cursor, Default::default()).unwrap();

		// and validate
		let actual = archive.read_file_by_name_or_crc(name, crc).map_err(|_| ());
		assert_eq!(expected, actual);
	}
}
