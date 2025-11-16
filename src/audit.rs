use std::fmt::Display;
use std::fmt::Formatter;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::Result;
use default_ext::DefaultExt;
use slint::SharedString;
use smol_str::SmolStr;
use zip::ZipArchive;

use crate::assethash::AssetHash;
use crate::info::AssetStatus;
use crate::info::Machine;
use crate::info::View;

pub struct Asset {
	pub kind: AssetKind,
	pub name: SharedString,
	pub size: Option<u64>,
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

pub struct AuditMessage {
	asset_name: SmolStr,
	details: AuditMessageDetails,
}

enum AuditMessageDetails {
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
		let roms = machine.roms().iter().map(|rom| Asset {
			kind: AssetKind::Rom,
			name: rom.name().into(),
			size: rom.size().into(),
			asset_hash: rom.asset_hash(),
			status: rom.status(),
			is_optional: rom.is_optional(),
		});
		let disks = machine.disks().iter().map(|disk| Asset {
			kind: AssetKind::Disk,
			name: disk.name().into(),
			size: None,
			asset_hash: disk.asset_hash(),
			status: disk.status(),
			is_optional: disk.is_optional(),
		});
		let samples = machine.samples().iter().map(|sample| Asset {
			kind: AssetKind::Sample,
			name: sample.name().into(),
			size: None,
			asset_hash: AssetHash::default(),
			status: AssetStatus::Good,
			is_optional: false,
		});
		roms.chain(disks).chain(samples).collect()
	}

	pub fn run_audit(
		&self,
		machine_names: &[impl AsRef<str>],
		rom_paths: &[impl AsRef<Path>],
		sample_paths: &[impl AsRef<Path>],
	) -> Vec<AuditMessage> {
		match self.kind {
			AssetKind::Rom => audit_single(
				&self.name,
				self.size,
				&self.asset_hash,
				self.status,
				self.is_optional,
				machine_names,
				rom_paths,
			),
			AssetKind::Disk => todo!(),
			AssetKind::Sample => audit_single(
				&self.name,
				self.size,
				&self.asset_hash,
				self.status,
				self.is_optional,
				machine_names,
				sample_paths,
			),
		}
	}
}

impl AuditMessage {
	pub fn severity(&self) -> AuditSeverity {
		self.details.severity()
	}
}

impl Display for AuditMessage {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		write!(f, "{} {}", self.asset_name, self.details)
	}
}

impl AuditMessageDetails {
	pub fn severity(&self) -> AuditSeverity {
		match self {
			Self::NotFoundButOptional | Self::NeedsRedump | Self::NoGoodDump => AuditSeverity::Info,
			Self::WrongLength { .. } | Self::WrongChecksums { .. } => AuditSeverity::Warning,
			Self::NotFound | Self::NotFoundNoGoodDump => AuditSeverity::Fail,
		}
	}
}

impl Display for AuditMessageDetails {
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

enum PathType {
	Dir,
	Zip,
}

fn audit_single(
	asset_name: &str,
	expected_size: Option<u64>,
	expected_asset_hash: &AssetHash,
	status: AssetStatus,
	is_optional: bool,
	machine_names: &[impl AsRef<str>],
	paths: &[impl AsRef<Path>],
) -> Vec<AuditMessage> {
	machine_names
		.iter()
		.flat_map(|machine_name| paths.iter().map(|path| (machine_name.as_ref(), path.as_ref())))
		.flat_map(|(machine_name, path)| [(machine_name, path, PathType::Dir), (machine_name, path, PathType::Zip)])
		.filter_map(|(machine_name, path, path_type)| {
			try_audit(
				asset_name,
				expected_size,
				expected_asset_hash,
				status,
				machine_name,
				path,
				path_type,
			)
			.ok()
		})
		.next()
		.unwrap_or_else(|| {
			let result = match (status, is_optional) {
				(AssetStatus::NoDump, _) => AuditMessageDetails::NotFoundNoGoodDump,
				(_, false) => AuditMessageDetails::NotFound,
				(_, true) => AuditMessageDetails::NotFoundButOptional,
			};
			vec![result]
		})
		.into_iter()
		.map(|details| {
			let asset_name = asset_name.into();
			AuditMessage { asset_name, details }
		})
		.collect::<Vec<_>>()
}

fn try_audit(
	asset_name: &str,
	expected_size: Option<u64>,
	expected_asset_hash: &AssetHash,
	status: AssetStatus,
	machine_name: &str,
	path: &Path,
	path_type: PathType,
) -> Result<Vec<AuditMessageDetails>> {
	let mut results = match path_type {
		PathType::Dir => {
			let path = path.join(machine_name).join(asset_name);
			let mut file = File::open(path)?;
			let actual_size = file.metadata()?.len();
			audit_loaded_file(&mut file, expected_size, expected_asset_hash, actual_size)
		}
		PathType::Zip => {
			let mut path = path.join(machine_name);
			path.set_extension("zip");
			let file = File::open(path)?;
			let mut zip_archive = ZipArchive::new(file)?;
			let mut zip_file = zip_archive.by_name(asset_name)?;
			let actual_size = zip_file.size();
			audit_loaded_file(&mut zip_file, expected_size, expected_asset_hash, actual_size)
		}
	}?;

	match status {
		AssetStatus::Good => {}
		AssetStatus::BadDump => results.push(AuditMessageDetails::NeedsRedump),
		AssetStatus::NoDump => results.push(AuditMessageDetails::NoGoodDump),
	}

	Ok(results)
}

fn audit_loaded_file(
	file: &mut dyn Read,
	expected_size: Option<u64>,
	expected_asset_hash: &AssetHash,
	actual_size: u64,
) -> Result<Vec<AuditMessageDetails>> {
	let mut results: Vec<AuditMessageDetails> = Vec::new();

	if let Some(expected) = expected_size
		&& expected != actual_size
	{
		let msg = AuditMessageDetails::WrongLength {
			expected,
			found: actual_size,
		};
		results.push(msg);
	}

	if !expected_asset_hash.is_default() {
		let found = AssetHash::calculate(file)?;
		if *expected_asset_hash != found {
			let msg = AuditMessageDetails::WrongChecksums {
				expected: *expected_asset_hash,
				found,
			};
			results.push(msg);
		}
	}

	Ok(results)
}
