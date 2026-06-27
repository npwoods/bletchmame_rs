use std::any::Any;
use std::cell::RefCell;
use std::ffi::OsString;
use std::sync::Arc;

use slint::Image;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::ToSharedString;
use slint::VecModel;
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
	icons: AuditIcons,
	notify: ModelNotify,
}

#[derive(Debug)]
struct AuditIcons {
	rom: Image,
	disk: Image,
	sample: Image,
	audit_pending: Image,
	audit_warning: Image,
	audit_failed: Image,
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
		let icons = AuditIcons::new(icons);

		Self {
			assets,
			rom_paths,
			sample_paths,
			audit_results,
			icons,
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

	pub fn get_model(model: &ModelRc<crate::ui::AuditAsset>) -> &Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for AuditModel {
	type Data = crate::ui::AuditAsset;

	fn row_count(&self) -> usize {
		self.assets.len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let asset = self.assets.get(row)?;
		let ui_asset = make_ui_asset(asset, self.audit_results.borrow()[row].as_ref(), &self.icons);
		Some(ui_asset)
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

impl AuditIcons {
	pub fn new(icons: Icons<'_>) -> Self {
		Self {
			rom: icons.get_rom(),
			disk: icons.get_harddisk(),
			sample: icons.get_sample(),
			audit_pending: icons.get_audit_pending(),
			audit_warning: icons.get_audit_warning(),
			audit_failed: icons.get_audit_failed(),
		}
	}
}

fn make_ui_asset(asset: &Asset, audit_result: Option<&AuditResult>, icons: &AuditIcons) -> crate::ui::AuditAsset {
	let (browse_action, max_severity, audit_messages) = {
		let max_severity = audit_result.map(AuditResult::severity);
		let audit_messages = audit_result
			.map(|r| r.messages.as_ref())
			.unwrap_or_default()
			.iter()
			.map(|r| r.to_shared_string())
			.collect::<Vec<_>>();
		let audit_messages = VecModel::from(audit_messages);
		let audit_messages = ModelRc::new(audit_messages);
		let browse_action = audit_result
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
		(browse_action, max_severity, audit_messages)
	};

	let icon = match asset.kind {
		AssetKind::Rom => icons.rom.clone(),
		AssetKind::Disk => icons.disk.clone(),
		AssetKind::Sample => icons.sample.clone(),
	};
	let overlay = match max_severity {
		None => icons.audit_pending.clone(),
		Some(AuditSeverity::Info) => Image::default(),
		Some(AuditSeverity::Warning) => icons.audit_warning.clone(),
		Some(AuditSeverity::Fail) => icons.audit_failed.clone(),
	};

	let name = asset.name.clone();
	let size = asset.size.map(|s| s.to_shared_string()).unwrap_or_default();
	crate::ui::AuditAsset {
		icon,
		overlay,
		name,
		size,
		audit_messages,
		browse_action,
	}
}
