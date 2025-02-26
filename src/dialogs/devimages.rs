use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::LogicalPosition;
use slint::ModelRc;
use slint::Weak;

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::devimageconfig::DevicesImagesConfig;
use crate::devimageconfig::EntryDetails;
use crate::dialogs::SingleResult;
use crate::guiutils::MenuingType;
use crate::guiutils::menuing::MenuDesc;
use crate::guiutils::menuing::MenuExt;
use crate::guiutils::modal::Modal;
use crate::models::devimages::DevicesAndImagesModel;
use crate::platform::WindowExt;
use crate::status::Status;
use crate::ui::DevicesAndImagesDialog;
use crate::ui::DevicesAndImagesState;

pub async fn dialog_devices_and_images(
	parent: Weak<impl ComponentHandle + 'static>,
	diconfig: DevicesImagesConfig,
	status_update_channel: Channel<Status>,
	invoke_command: impl Fn(AppCommand) + 'static,
	menuing_type: MenuingType,
) {
	// prepare the dialog
	let modal = Modal::new(&parent.unwrap(), || DevicesAndImagesDialog::new().unwrap());
	let single_result = SingleResult::default();

	// set up the model
	let model = DevicesAndImagesModel::new(diconfig);
	let none_string = model.none_string.clone();
	let model = ModelRc::new(model);
	let state = DevicesAndImagesState {
		entries: model.clone(),
		none_string,
	};
	modal.dialog().set_state(state);

	// set up the "ok" button
	let signaller = single_result.signaller();
	modal.dialog().on_ok_clicked(move || {
		signaller.signal(());
	});

	// set up the "apply changes" button
	let model_clone = model.clone();
	modal.dialog().on_apply_changes_clicked(move || {
		let model = DevicesAndImagesModel::get_model(&model_clone);
		let changed_slots = model.with_diconfig(|diconfig| diconfig.changed_slots(true));
		let command = AppCommand::ChangeSlots(changed_slots);
		invoke_command(command);
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
	let model_clone = model.clone();
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_entry_button_clicked(move |entry_index, point| {
		let dialog = dialog_weak.unwrap();
		let model = DevicesAndImagesModel::get_model(&model_clone);
		let entry_index = entry_index.try_into().unwrap();
		entry_popup_menu(&dialog, model, menuing_type, entry_index, point);
	});

	// subscribe to status changes
	let model_clone = model.clone();
	let dialog_weak = modal.dialog().as_weak();
	let _subscription = status_update_channel.subscribe(move |status| {
		// update the model
		let model = DevicesAndImagesModel::get_model(&model_clone);
		model.change_diconfig(|diconfig| Some(diconfig.update_status(status)));

		// update the dirty flag
		let dirty = model.with_diconfig(|diconfig| diconfig.is_dirty());
		dialog_weak.unwrap().set_config_dirty(dirty);
	});

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await;
}

fn entry_popup_menu(
	dialog: &DevicesAndImagesDialog,
	model: &DevicesAndImagesModel,
	menuing_type: MenuingType,
	entry_index: usize,
	point: LogicalPosition,
) {
	let menu_items = model.with_diconfig(|diconfig| {
		let entry = diconfig.entry(entry_index).unwrap();
		let EntryDetails::Image { filename } = &entry.details else {
			unreachable!();
		};

		let load_command = {
			let tag = entry.tag.to_string();
			let command = AppCommand::LoadImageDialog { tag };
			Some(command.into())
		};
		let connect_socket_command = {
			let tag = entry.tag.to_string();
			let command = AppCommand::ConnectToSocketDialog { tag };
			Some(command.into())
		};
		let unload_command = filename.is_some().then(|| {
			let tag = entry.tag.to_string();
			let command = AppCommand::UnloadImage { tag };
			command.into()
		});
		[
			MenuDesc::Item("Create Image...".into(), None),
			MenuDesc::Item("Load Image...".into(), load_command),
			MenuDesc::Item("Load Software List Part...".into(), None),
			MenuDesc::Item("Connect To Socket...".into(), connect_socket_command),
			MenuDesc::Item("Unload".into(), unload_command),
		]
	});
	let popup_menu = MenuDesc::make_popup_menu(menu_items);

	match menuing_type {
		MenuingType::Native => {
			dialog.window().show_popup_menu(&popup_menu, point);
		}
		MenuingType::Slint => {
			let entries = popup_menu.slint_menu_entries(None);
			dialog.invoke_show_context_menu(entries, point);
		}
	}
}
