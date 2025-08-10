use std::any::Any;
use std::path::Path;
use std::rc::Rc;

use anyhow::Result;
use more_asserts::assert_lt;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::ToSharedString;
use slint::VecModel;
use tokio::sync::mpsc;

use crate::dialogs::SenderExt;
use crate::dialogs::file::load_file_dialog;
use crate::guiutils::modal::ModalStack;
use crate::importmameini::ImportMameIni;
use crate::prefs::PrefsPaths;
use crate::ui::ImportMameIniDialog;
use crate::ui::ImportMameIniDialogEntry;

const MAME_INI_EXTENSION: &str = "ini";
const MAME_INI_FILE_TYPES: &[(Option<&str>, &str)] = &[(None, MAME_INI_EXTENSION)];

pub async fn dialog_import_mame_ini(
	modal_stack: ModalStack,
	prefs_paths: impl AsRef<PrefsPaths>,
) -> Result<Option<Rc<ImportMameIni>>> {
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

	// prepare the modal dialog
	let modal = modal_stack.modal(|| ImportMameIniDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// read the options and create the model
	let dialog_weak = modal.dialog().as_weak();
	let update_cb = move |import_mame_ini: &ImportMameIni| {
		let can_apply = import_mame_ini.can_apply();
		dialog_weak.unwrap().set_ok_enabled(can_apply);
	};
	let model = ImportMameIniModel::new(&ini_path, prefs_paths, update_cb)?;
	let model = ModelRc::new(model);

	// add the model
	let model_clone = model.clone();
	modal.dialog().set_entries(model_clone);

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
	let result = accepted.then(move || {
		let model = ImportMameIniModel::get_model(&model);
		model.import_ini.clone()
	});
	Ok(result)
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
	import_ini: Rc<ImportMameIni>,
	update_cb: Box<dyn Fn(&ImportMameIni)>,
	notify: ModelNotify,
}

impl ImportMameIniModel {
	pub fn new(
		path: impl AsRef<Path>,
		prefs_paths: &PrefsPaths,
		update_cb: impl Fn(&ImportMameIni) + 'static,
	) -> Result<Self> {
		let import_ini = ImportMameIni::read_mame_ini(path, prefs_paths)?;
		let import_ini = Rc::new(import_ini);
		let update_cb = Box::new(update_cb) as Box<_>;

		let result = Self {
			import_ini,
			update_cb,
			notify: Default::default(),
		};
		result.invoke_update_cb();
		Ok(result)
	}

	fn invoke_update_cb(&self) {
		(self.update_cb)(&self.import_ini);
	}

	pub fn get_model(model: &ModelRc<ImportMameIniDialogEntry>) -> &Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for ImportMameIniModel {
	type Data = ImportMameIniDialogEntry;

	fn row_count(&self) -> usize {
		self.import_ini.entries().len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let entry = self.import_ini.entries().get(row)?;

		let path_type = entry.opt.path_type.to_shared_string();
		let path = entry.opt.value.to_shared_string();

		let dispositions = entry
			.dispositions
			.iter()
			.map(|d| d.to_shared_string())
			.collect::<Vec<_>>();
		let dispositions = VecModel::from(dispositions);
		let dispositions = ModelRc::new(dispositions);

		let current_disposition_index = entry.current_disposition_index.get().try_into().unwrap();

		let result = Self::Data {
			path_type,
			path,
			current_disposition_index,
			dispositions,
		};
		Some(result)
	}

	fn set_row_data(&self, row: usize, data: Self::Data) {
		// find the entry
		let entry = self.import_ini.entries().get(row).unwrap();

		// set the new disposition index
		let current_disposition_index = data.current_disposition_index.try_into().unwrap();
		assert_lt!(current_disposition_index, entry.dispositions.len());
		entry.current_disposition_index.set(current_disposition_index);

		// notify
		self.notify.row_changed(row);
		self.invoke_update_cb();
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}
