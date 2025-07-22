use std::any::Any;
use std::path::Path;

use anyhow::Result;
use slint::VecModel;
use slint::{CloseRequestResponse, Model, ModelNotify, ModelRc, ModelTracker, ToSharedString};
use tokio::sync::mpsc;

use crate::dialogs::SenderExt;
use crate::dialogs::file::load_file_dialog;
use crate::guiutils::modal::ModalStack;
use crate::importmameini::Disposition;
use crate::importmameini::ImportMameIni;
use crate::prefs::PrefsPaths;
use crate::ui::ImportMameIniDialog;
use crate::ui::ImportMameIniDialogEntry;

const MAME_INI_EXTENSION: &str = "ini";
const MAME_INI_FILE_TYPES: &[(Option<&str>, &str)] = &[(None, MAME_INI_EXTENSION)];

pub async fn dialog_import_mame_ini(
	modal_stack: ModalStack,
	prefs_paths: impl AsRef<PrefsPaths>,
) -> Result<Option<ImportMameIni>> {
	// if we can find a MAME.ini, lets point to it
	let prefs_paths = prefs_paths.as_ref();
	let (initial_dir, initial_file) = prefs_paths
		.mame_executable
		.as_deref()
		.map(get_initial_paths)
		.unwrap_or_default();

	// show the file dialog
	let parent = modal_stack.top();
	let file_types = MAME_INI_FILE_TYPES;
	let ini_path = load_file_dialog(parent, "Import MAME INI", file_types, initial_dir, initial_file).await;
	let Some(ini_path) = ini_path else {
		return Ok(None);
	};

	// read the options and create the model
	let model = ImportMameIniModel::new(&ini_path, prefs_paths)?;
	let model = ModelRc::new(model);

	// prepare the modal dialog
	let modal = modal_stack.modal(|| ImportMameIniDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// add the model
	modal.dialog().set_entries(model);

	// set up the "ok" button
	let tx_clone = tx.clone();
	modal.dialog().on_ok_clicked(move || {
		tx_clone.signal(true);
	});

	// set up the "cancel" button
	let tx_clone = tx.clone();
	modal.dialog().on_cancel_clicked(move || {
		tx_clone.signal(false);
	});

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(false);
		CloseRequestResponse::KeepWindowShown
	});

	// present the modal dialog
	let accepted = modal.run(async { rx.recv().await.unwrap() }).await;

	// if the user hit "ok", return
	Ok(accepted.then(|| todo!()))
}

fn get_initial_paths(mame_executable_path: &str) -> (Option<&'_ Path>, Option<&'_ str>) {
	let mame_executable_path = Path::new(mame_executable_path);
	let initial_dir = mame_executable_path.parent().and_then(|x| x.is_dir().then_some(x));

	let initial_file = initial_dir.and_then(|initial_dir| {
		let initial_dir = Path::new(initial_dir);
		let initial_file = "mame.ini";
		initial_dir.join(initial_file).is_file().then_some(initial_file)
	});

	(initial_dir, initial_file)
}

struct ImportMameIniModel {
	import_ini: ImportMameIni,
	notify: ModelNotify,
}

impl ImportMameIniModel {
	pub fn new(path: impl AsRef<Path>, prefs_paths: &PrefsPaths) -> Result<Self> {
		let import_ini = ImportMameIni::read_mame_ini(path, prefs_paths)?;
		let result = Self {
			import_ini,
			notify: Default::default(),
		};
		Ok(result)
	}
}

impl Model for ImportMameIniModel {
	type Data = ImportMameIniDialogEntry;

	fn row_count(&self) -> usize {
		self.import_ini.entries().len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let (option, disposition) = self.import_ini.entries().get(row)?;

		let path_type = option.path_type.to_shared_string();
		let path = option.value.to_shared_string();
		let current_disposition = disposition.get().to_shared_string();

		let dispositions = if disposition.get() == Disposition::AlreadyPresent {
			vec![Disposition::AlreadyPresent]
		} else if option.path_type.is_multi() {
			vec![Disposition::Ignore, Disposition::Supplement]
		} else {
			vec![Disposition::Ignore, Disposition::Replace]
		};
		let dispositions = dispositions
			.into_iter()
			.map(|d| d.to_shared_string())
			.collect::<Vec<_>>();
		let dispositions = VecModel::from(dispositions);
		let dispositions = ModelRc::new(dispositions);

		let result = Self::Data {
			path_type,
			path,
			current_disposition,
			dispositions,
		};
		Some(result)
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}
