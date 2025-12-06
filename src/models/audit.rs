use std::any::Any;
use std::cell::RefCell;
use std::ffi::OsString;
use std::sync::Arc;

use itertools::Itertools;
use slint::Image;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::ToSharedString;
use smol_str::SmolStr;
use tokio::task::spawn_blocking;

use crate::action::Action;
use crate::audit::Asset;
use crate::audit::AssetKind;
use crate::audit::AuditResult;
use crate::audit::AuditSeverity;
use crate::audit::PathType;
use crate::mconfig::MachineConfig;
use crate::ui::Icons;

pub struct AuditModel {
	assets: Arc<[Asset]>,
	rom_paths: Arc<[SmolStr]>,
	sample_paths: Arc<[SmolStr]>,
	audit_results: RefCell<Box<[Option<AuditResult>]>>,

	icon_rom: Image,
	icon_disk: Image,
	icon_sample: Image,
	icon_audit_pending: Image,
	icon_audit_warning: Image,
	icon_audit_failed: Image,

	notify: ModelNotify,
}

impl AuditModel {
	pub fn new(
		machine_config: &MachineConfig,
		rom_paths: Arc<[SmolStr]>,
		sample_paths: Arc<[SmolStr]>,
		icons: Icons<'_>,
	) -> Self {
		let assets = Asset::from_machine_config(machine_config)
			.into_iter()
			.collect::<Arc<[_]>>();
		let audit_results = (0..assets.len()).map(|_| None).collect::<Box<_>>();
		let audit_results = RefCell::new(audit_results);

		Self {
			assets,
			rom_paths,
			sample_paths,
			audit_results,
			icon_rom: icons.get_rom(),
			icon_disk: icons.get_harddisk(),
			icon_sample: icons.get_sample(),
			icon_audit_pending: icons.get_audit_pending(),
			icon_audit_warning: icons.get_audit_warning(),
			icon_audit_failed: icons.get_audit_failed(),
			notify: ModelNotify::default(),
		}
	}

	pub async fn run_audit(&self) {
		// everything is unknown when we start an audit
		for row in 0..self.assets.len() {
			let changed = {
				let mut audit_results = self.audit_results.borrow_mut();
				let changed = audit_results[row].is_some();
				audit_results[row] = None;
				changed
			};
			if changed {
				self.notify.row_changed(row);
			}
		}

		// now audit each of the assets
		for row in 0..self.assets.len() {
			let assets = self.assets.clone();
			let rom_paths = self.rom_paths.clone();
			let sample_paths = self.sample_paths.clone();
			let single_result = spawn_blocking(move || assets[row].run_audit(&rom_paths, &sample_paths))
				.await
				.unwrap();

			self.audit_results.borrow_mut()[row] = Some(single_result);
			self.notify.row_changed(row);
		}
	}

	pub fn get_model(model: &ModelRc<crate::ui::Asset>) -> &Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for AuditModel {
	type Data = crate::ui::Asset;

	fn row_count(&self) -> usize {
		self.assets.len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let asset = self.assets.get(row)?;
		let (browse_command, max_severity, tooltip_text) = {
			let audit_results = self.audit_results.borrow();
			let audit_result = &audit_results[row];
			let max_severity = audit_result.as_ref().map(AuditResult::severity);
			let tooltip_text = audit_result
				.as_ref()
				.map(|r| r.messages.as_ref())
				.unwrap_or_default()
				.iter()
				.map(|r| format!("{} {}", &asset.name, r))
				.join("\n")
				.into();
			let browse_command = audit_result
				.as_ref()
				.and_then(|r| r.path.as_ref())
				.map(|(path, path_type)| {
					let action = match path_type {
						PathType::File => Action::ShowFile(path.clone().into()),
						PathType::Zip => Action::Launch(OsString::from(path).into()),
					};
					action.encode_for_slint()
				})
				.unwrap_or_default();
			(browse_command, max_severity, tooltip_text)
		};

		let icon = match asset.kind {
			AssetKind::Rom => self.icon_rom.clone(),
			AssetKind::Disk => self.icon_disk.clone(),
			AssetKind::Sample => self.icon_sample.clone(),
		};
		let overlay = match max_severity {
			None => self.icon_audit_pending.clone(),
			Some(AuditSeverity::Info) => Image::default(),
			Some(AuditSeverity::Warning) => self.icon_audit_warning.clone(),
			Some(AuditSeverity::Fail) => self.icon_audit_failed.clone(),
		};

		let name = asset.name.clone();
		let size = asset.size.map(|s| s.to_shared_string()).unwrap_or_default();
		let data = Self::Data {
			icon,
			overlay,
			name,
			size,
			tooltip_text,
			browse_command,
		};
		Some(data)
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}
