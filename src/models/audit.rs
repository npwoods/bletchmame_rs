use std::any::Any;
use std::cell::RefCell;
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

use crate::audit::Asset;
use crate::audit::AssetKind;
use crate::audit::AuditMessage;
use crate::audit::AuditSeverity;
use crate::info::Machine;
use crate::ui::Icons;

pub struct AuditModel {
	machine_names: Arc<[SmolStr]>,
	assets: Arc<[Asset]>,
	rom_paths: Arc<[SmolStr]>,
	sample_paths: Arc<[SmolStr]>,

	#[allow(clippy::type_complexity)]
	audit_results: RefCell<Box<[Option<Box<[AuditMessage]>>]>>,

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
		machine: Machine<'_>,
		rom_paths: Arc<[SmolStr]>,
		sample_paths: Arc<[SmolStr]>,
		icons: Icons<'_>,
	) -> Self {
		let machine_names = [Some(machine.name()), machine.clone_of().map(|x| x.name())]
			.iter()
			.flatten()
			.copied()
			.map(SmolStr::from)
			.collect::<Arc<[_]>>();

		let assets = Asset::from_machine(machine).into_iter().collect::<Arc<[_]>>();
		let audit_results = (0..assets.len()).map(|_| None).collect::<Box<_>>();
		let audit_results = RefCell::new(audit_results);

		Self {
			machine_names,
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
			let machine_names = self.machine_names.clone();
			let rom_paths = self.rom_paths.clone();
			let sample_paths = self.sample_paths.clone();
			let single_results =
				spawn_blocking(move || assets[row].run_audit(&machine_names, &rom_paths, &sample_paths))
					.await
					.unwrap();

			self.audit_results.borrow_mut()[row] = Some(single_results.into());
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
		let (max_severity, tooltip_text) = {
			let audit_results = self.audit_results.borrow();
			let audit_result = audit_results[row].as_deref();
			let max_severity = audit_result.map(|r| {
				r.iter()
					.map(AuditMessage::severity)
					.max()
					.unwrap_or(AuditSeverity::Info)
			});
			let tooltip_text = audit_result
				.unwrap_or_default()
				.iter()
				.map(|r| r.to_string())
				.join("\n")
				.into();
			(max_severity, tooltip_text)
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
