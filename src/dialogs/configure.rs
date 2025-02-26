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
use crate::mconfig::MachineConfig;
use crate::models::devimages::DevicesAndImagesModel;
use crate::prefs::PrefsMachineItem;
use crate::ui::ConfigureDialog;
use crate::ui::DevicesAndImagesState;

pub async fn dialog_configure(
	parent: Weak<impl ComponentHandle + 'static>,
	info_db: Rc<InfoDb>,
	item: PrefsMachineItem,
) -> Option<PrefsMachineItem> {
	// prepare the dialog
	let modal = Modal::new(&parent.unwrap(), || ConfigureDialog::new().unwrap());
	let single_result = SingleResult::default();

	// look up the machine and create the devimages config
	match MachineConfig::from_machine_name_and_slots(info_db, &item.machine_name, &item.slots) {
		Ok(machine_config) => {
			let diconfig = DevicesImagesConfig::from(machine_config);
			let machine = diconfig.machine().unwrap();

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
			let model_clone = model.clone();
			modal.dialog().on_ok_clicked(move || {
				let model = DevicesAndImagesModel::get_model(&model_clone);
				let result = machine_item_from_model(model);
				signaller.signal(Some(result));
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
		}
		Err(e) => {
			let text = format!("{e}").into();
			modal.dialog().set_dev_images_error(text);
		}
	}

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await
}

fn machine_item_from_model(model: &DevicesAndImagesModel) -> PrefsMachineItem {
	model.with_diconfig(|diconfig| {
		let machine_name = diconfig.machine().unwrap().name().to_string();
		let slots = diconfig.changed_slots(false);
		PrefsMachineItem { machine_name, slots }
	})
}
