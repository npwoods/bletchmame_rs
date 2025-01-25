use std::any::Any;
use std::borrow::Cow;
use std::cell::RefCell;
use std::path::Path;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::LogicalPosition;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::devimageconfig::DevicesImagesConfig;
use crate::devimageconfig::EntryDetails;
use crate::dialogs::SingleResult;
use crate::guiutils::menuing::MenuDesc;
use crate::guiutils::menuing::MenuExt;
use crate::guiutils::modal::Modal;
use crate::guiutils::MenuingType;
use crate::platform::WindowExt;
use crate::status::Status;
use crate::ui::DeviceAndImageEntry;
use crate::ui::DevicesAndImagesDialog;

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
	let none_string = SharedString::from("<<none>>");
	let model = DevicesAndImagesModel {
		diconfig: RefCell::new(diconfig),
		dialog_weak: modal.dialog().as_weak(),
		menuing_type,
		none_string: none_string.clone(),
		notify: ModelNotify::default(),
	};
	let model = ModelRc::new(model);
	modal.dialog().set_entries(model.clone());
	modal.dialog().set_none_string(none_string);

	// set up the "ok" button
	let signaller = single_result.signaller();
	modal.dialog().on_ok_clicked(move || {
		signaller.signal(());
	});

	// set up the "apply changes" button
	let model_clone = model.clone();
	modal.dialog().on_apply_changes_clicked(move || {
		let model = DevicesAndImagesModel::get_model(&model_clone);
		let changed_slots = model.with_diconfig(DevicesImagesConfig::changed_slots);
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
			model.change_diconfig(|diconfig| {
				let tag = diconfig.entry(entry_index).unwrap().tag;
				Some(diconfig.set_slot_option(tag, new_option_name))
			});
		});
	let model_clone = model.clone();
	modal.dialog().on_entry_button_clicked(move |entry_index, point| {
		let model = DevicesAndImagesModel::get_model(&model_clone);
		let entry_index = entry_index.try_into().unwrap();
		entry_popup_menu(model, entry_index, point);
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

fn entry_popup_menu(model: &DevicesAndImagesModel, entry_index: usize, point: LogicalPosition) {
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
		let unload_command = filename.is_some().then(|| {
			let tag = entry.tag.to_string();
			let command = AppCommand::UnloadImage { tag };
			command.into()
		});
		[
			MenuDesc::Item("Create Image...".into(), None),
			MenuDesc::Item("Load Image...".into(), load_command),
			MenuDesc::Item("Load Software List Part...".into(), None),
			MenuDesc::Item("Unload".into(), unload_command),
		]
	});
	let popup_menu = MenuDesc::make_popup_menu(menu_items);

	let dialog = model.dialog_weak.unwrap();
	match model.menuing_type {
		MenuingType::Native => {
			dialog.window().show_popup_menu(&popup_menu, point);
		}
		MenuingType::Slint => {
			let entries = popup_menu.slint_menu_entries(None);
			dialog.invoke_show_context_menu(entries, point);
		}
	}
}

struct DevicesAndImagesModel {
	diconfig: RefCell<DevicesImagesConfig>,
	dialog_weak: Weak<DevicesAndImagesDialog>,
	menuing_type: MenuingType,
	none_string: SharedString,
	notify: ModelNotify,
}

impl DevicesAndImagesModel {
	pub fn change_diconfig(&self, callback: impl FnOnce(&DevicesImagesConfig) -> Option<DevicesImagesConfig>) {
		// update the config in our RefCell
		let range = {
			let mut diconfig = self.diconfig.borrow_mut();
			let new_diconfig = callback(&diconfig);
			if let Some(new_diconfig) = new_diconfig {
				let range = diconfig.identify_changed_rows(&new_diconfig);
				*diconfig = new_diconfig;
				range
			} else {
				Some(Vec::new())
			}
		};

		// notify row changes (if any)
		if let Some(range) = range {
			for row in range {
				self.notify.row_changed(row);
			}
		} else {
			self.notify.reset();
		}
	}

	pub fn with_diconfig<R>(&self, callback: impl FnOnce(&DevicesImagesConfig) -> R) -> R {
		let diconfig = self.diconfig.borrow();
		callback(&diconfig)
	}

	pub fn get_model(model: &impl Model) -> &'_ Self {
		model.as_any().downcast_ref::<DevicesAndImagesModel>().unwrap()
	}
}

impl Model for DevicesAndImagesModel {
	type Data = DeviceAndImageEntry;

	fn row_count(&self) -> usize {
		self.diconfig.borrow().entry_count()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		// retrieve the entry
		let diconfig = self.diconfig.borrow();
		let entry = diconfig.entry(row)?;

		// basic indent and display tag stuff
		let display_tag = SharedString::from(entry.subtag);
		let indent = entry.indent.try_into().unwrap();

		// now figure out the slot/image-specific details
		let (options, current_option_index, filename) = match entry.details {
			EntryDetails::Slot {
				options,
				current_option_index,
			} => {
				let options = options
					.into_iter()
					.map(|opt| {
						if let Some(name) = opt.name {
							let name = SharedString::from(name.as_ref());
							if let Some(desc) = opt.description {
								(name.clone(), format!("{desc} ({name})").into())
							} else {
								(name.clone(), name)
							}
						} else {
							("".into(), self.none_string.clone())
						}
					})
					.collect::<Vec<_>>();
				let current_option_index = current_option_index.try_into().unwrap();
				(options, current_option_index, "".into())
			}
			EntryDetails::Image { filename } => {
				let filename = filename.map(|x| match Path::new(x).file_name() {
					Some(x) => x.to_string_lossy(),
					None => Cow::Borrowed(x),
				});
				let filename = filename.unwrap_or_default().as_ref().into();
				(Vec::default(), -1, filename)
			}
		};

		let (option_names, option_descriptions): (Vec<_>, Vec<_>) = options.into_iter().unzip();
		let option_names = VecModel::from(option_names);
		let option_names = ModelRc::new(option_names);
		let option_descriptions = VecModel::from(option_descriptions);
		let option_descriptions = ModelRc::new(option_descriptions);

		let result = DeviceAndImageEntry {
			display_tag,
			indent,
			option_names,
			option_descriptions,
			current_option_index,
			filename,
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
