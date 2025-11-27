use std::fmt::Display;
use std::fmt::Formatter;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use default_ext::DefaultExt;
use slint::SharedString;
use smol_str::SmolStr;
use tracing::debug;
use zip::ZipArchive;

use crate::assethash::AssetHash;
use crate::chd::chd_asset_hash;
use crate::info::AssetStatus;
use crate::info::Machine;
use crate::info::View;

pub struct Asset {
	pub kind: AssetKind,
	pub name: SharedString,
	pub size: Option<u64>,
	machine_names: Arc<[SmolStr]>,
	asset_hash: AssetHash,
	status: AssetStatus,
	is_optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct AuditResult {
	pub path: Option<(PathBuf, PathType)>,
	pub messages: Box<[AuditMessage]>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuditMessage {
	NotFound,
	NotFoundNoGoodDump,
	NotFoundButOptional,
	WrongLength { expected: u64, found: u64 },
	WrongChecksums { expected: AssetHash, found: AssetHash },
	NeedsRedump,
	NoGoodDump,
}

impl Asset {
	pub fn from_machine(machine: Machine<'_>) -> Vec<Self> {
		let mut results = Vec::new();
		Self::from_machine_internal(&mut results, machine);
		results
	}

	fn from_machine_internal(results: &mut Vec<Self>, machine: Machine<'_>) {
		let machine_names = [Some(machine.name()), machine.clone_of().map(|x| x.name())]
			.iter()
			.flatten()
			.copied()
			.map(SmolStr::from)
			.collect::<Arc<[_]>>();
		let roms = machine.roms().iter().map(|rom| Asset {
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
			is_optional: false,
		});
		results.extend(roms.chain(disks).chain(samples));

		for machine in machine.device_refs().iter().filter_map(|dr| dr.machine()) {
			Self::from_machine_internal(results, machine);
		}
	}

	pub fn run_audit<P>(&self, rom_paths: &[P], sample_paths: &[P]) -> AuditResult
	where
		P: AsRef<Path>,
	{
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
			AssetKind::Rom => (rom_paths, true, hash_func_rom as HashFunc),
			AssetKind::Sample => (sample_paths, true, hash_func_sample as HashFunc),
			AssetKind::Disk => (rom_paths, false, hash_func_disk as HashFunc),
		};

		// now do the heavy lifting
		audit_single(
			&self.name,
			self.size,
			&self.asset_hash,
			self.status,
			self.is_optional,
			self.machine_names.as_ref(),
			asset_paths,
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
			Self::NotFound => writeln!(f, "NOT FOUND")?,
			Self::NotFoundNoGoodDump => writeln!(f, "NOT FOUND - NO GOOD DUMP KNOWN")?,
			Self::NotFoundButOptional => writeln!(f, "NOT FOUND BUT OPTIONAL")?,
			Self::WrongLength { expected, found } => writeln!(f, "WRONG LENGTH (expected: {expected} found {found})")?,
			Self::WrongChecksums { expected, found } => {
				writeln!(f, "WRONG CHECKSUMS:")?;
				writeln!(f, "    EXPECTED: {expected}")?;
				writeln!(f, "       FOUND: {found}")?;
			}
			Self::NeedsRedump => writeln!(f, "NEEDS REDUMP")?,
			Self::NoGoodDump => writeln!(f, "NO GOOD DUMP KNOWN")?,
		}
		Ok(())
	}
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PathType {
	File,
	Zip,
}

#[allow(clippy::too_many_arguments)]
fn audit_single(
	asset_name: &str,
	expected_size: Option<u64>,
	expected_asset_hash: &AssetHash,
	status: AssetStatus,
	is_optional: bool,
	machine_names: &[impl AsRef<str>],
	paths: &[impl AsRef<Path>],
	support_archives: bool,
	hash_func: fn(&mut dyn Read) -> Result<AssetHash>,
) -> AuditResult {
	let path_types = if support_archives {
		[PathType::File, PathType::Zip].as_slice()
	} else {
		[PathType::File].as_slice()
	};

	machine_names
		.iter()
		.flat_map(|machine_name| paths.iter().map(|path| (machine_name.as_ref(), path.as_ref())))
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
	let (path, actual_size, actual_hash) = match path_type {
		PathType::File => {
			let path = path.join(machine_name).join(asset_name);
			let file_result = File::open(&path);
			debug!(?path_type, ?path, ?file_result, "try_audit(): Invoked File::open()");
			let mut file = file_result?;
			let actual_size = file.metadata()?.len();
			let actual_hash = (!expected_asset_hash.is_default())
				.then(|| hash_func(&mut file))
				.transpose()?;
			(path, actual_size, actual_hash)
		}
		PathType::Zip => {
			let mut path = path.join(machine_name);
			path.set_extension("zip");
			let file_result = File::open(&path);
			debug!(?path_type, ?path, ?file_result, "try_audit(): Invoked File::open()");
			let mut zip_archive = ZipArchive::new(file_result?)?;
			let mut zip_file = zip_archive.by_name(asset_name)?;
			let actual_size = zip_file.size();
			let actual_hash = (!expected_asset_hash.is_default())
				.then(|| hash_func(&mut zip_file))
				.transpose()?;
			(path, actual_size, actual_hash)
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
