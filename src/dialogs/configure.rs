use std::rc::Rc;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;

use crate::devimageconfig::DevicesImagesConfig;
use crate::dialogs::SingleResult;
use crate::guiutils::modal::Modal;
use crate::info::InfoDb;
use crate::info::View;
use crate::models::devimages::DevicesAndImagesModel;
use crate::ui::ConfigureDialog;
use crate::ui::DevicesAndImagesState;

pub async fn dialog_configure(parent: Weak<impl ComponentHandle + 'static>, info_db: Rc<InfoDb>, machine_name: String) {
	// look up the machine and create the devimages config
	let diconfig = DevicesImagesConfig::with_machine_name(info_db, Some(&machine_name)).unwrap();
	let machine = diconfig.machine().unwrap();

	// prepare the dialog
	let modal = Modal::new(&parent.unwrap(), || ConfigureDialog::new().unwrap());
	let single_result = SingleResult::default();

	// ram options
	let ram_options = machine
		.ram_options()
		.iter()
		.map(|opt| SharedString::from(opt.size().to_string()))
		.collect::<Vec<_>>();
	let ram_options = VecModel::from(ram_options);
	let ram_options = ModelRc::new(ram_options);
	modal.dialog().set_ram_sizes_model(ram_options);

	// set up the devices and images model
	let model = DevicesAndImagesModel::new(diconfig);
	let none_string = model.none_string.clone();
	let model = ModelRc::new(model);
	let state = DevicesAndImagesState {
		entries: model.clone(),
		none_string,
	};
	modal.dialog().set_dev_images_state(state);

	// set up the "ok" button
	let signaller = single_result.signaller();
	modal.dialog().on_ok_clicked(move || {
		signaller.signal(());
	});

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(());
		CloseRequestResponse::KeepWindowShown
	});

	// set up callbacks
	let model_clone = model.clone();
	modal
		.dialog()
		.on_entry_option_changed(move |entry_index, new_option_name| {
			let entry_index = entry_index.try_into().unwrap();
			let new_option_name = (!new_option_name.is_empty()).then_some(new_option_name.as_str());
			let model = DevicesAndImagesModel::get_model(&model_clone);
			model.set_slot_entry_option(entry_index, new_option_name);
		});

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await;
}
