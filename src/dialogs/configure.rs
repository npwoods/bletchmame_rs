use std::iter::once;
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

	// find the machine
	let machine_index = info_db.machines().find_index(&item.machine_name).unwrap();

	// look up the machine and create the devimages config
	match MachineConfig::from_machine_index_and_slots(info_db.clone(), machine_index, &item.slots) {
		Ok(machine_config) => {
			let diconfig = DevicesImagesConfig::from(machine_config);

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
			let dialog_weak = modal.dialog().as_weak();
			modal.dialog().on_ok_clicked(move || {
				let model = DevicesAndImagesModel::get_model(&model_clone);
				let dialog = dialog_weak.unwrap();
				let result = machine_item_from_model(model, &dialog);
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

	// set up RAM size options
	let ram_options = info_db.machines().get(machine_index).unwrap().ram_options();
	if !ram_options.is_empty() {
		let default_index = ram_options
			.iter()
			.position(|x| x.is_default())
			.expect("expected a default RAM option");
		let default_text = format!(
			"Default ({})",
			ram_size_display_text(ram_options.get(default_index).unwrap().size())
		);
		let ram_option_texts = once(SharedString::from(default_text))
			.chain(ram_options.iter().map(|opt| ram_size_display_text(opt.size()).into()))
			.collect::<Vec<_>>();
		let ram_option_texts = VecModel::from(ram_option_texts);
		let ram_option_texts = ModelRc::new(ram_option_texts);
		modal.dialog().set_ram_sizes_model(ram_option_texts);

		// current RAM size option
		let index = item
			.ram_size
			.and_then(|ram_size| ram_options.iter().position(|x| x.size() == ram_size))
			.map(|idx| idx + 1)
			.unwrap_or_default();
		modal.dialog().set_ram_sizes_index(index.try_into().unwrap());
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

fn machine_item_from_model(model: &DevicesAndImagesModel, dialog: &ConfigureDialog) -> PrefsMachineItem {
	model.with_diconfig(|diconfig| {
		let machine = diconfig.machine().unwrap();
		let machine_name = machine.name().to_string();
		let slots = diconfig.changed_slots(false);
		let ram_sizes_index = dialog.get_ram_sizes_index();
		let ram_size = usize::try_from(ram_sizes_index - 1)
			.ok()
			.map(|index| machine.ram_options().get(index).unwrap().size());
		PrefsMachineItem {
			machine_name,
			slots,
			ram_size,
		}
	})
}

fn ram_size_display_text(ram_size: u64) -> String {
	ram_size.to_string()
}
