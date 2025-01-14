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
		signaller.signal(true);
	});

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(false);
		CloseRequestResponse::KeepWindowShown
	});

	// set up callbacks
	modal.dialog().on_entry_option_changed(|entry_index, option_index| {
		todo!("on_entry_option_changed(): entry_index={entry_index} option_index={option_index}");
	});
	let model_clone = model.clone();
	modal.dialog().on_entry_button_clicked(move |entry_index, point| {
		let model = &model_clone.as_any().downcast_ref::<DevicesAndImagesModel>().unwrap();
		let entry_index = entry_index.try_into().unwrap();
		entry_popup_menu(model, entry_index, point);
	});

	// subscribe to status changes
	let model_clone = model.clone();
	let _subscription = status_update_channel.subscribe(move |status| {
		let model = &model_clone.as_any().downcast_ref::<DevicesAndImagesModel>().unwrap();
		model.update_status(status);
	});

	// present the modal dialog
	let _accepted = modal.run(async { single_result.wait().await }).await;
}

fn entry_popup_menu(model: &DevicesAndImagesModel, entry_index: usize, point: LogicalPosition) {
	let menu_items = {
		let diconfig = model.diconfig.borrow();
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
	};
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
	pub diconfig: RefCell<DevicesImagesConfig>,
	dialog_weak: Weak<DevicesAndImagesDialog>,
	menuing_type: MenuingType,
	none_string: SharedString,
	notify: ModelNotify,
}

impl DevicesAndImagesModel {
	pub fn update_status(&self, status: &Status) {
		let range = {
			let mut diconfig = self.diconfig.borrow_mut();
			let (new_diconfig, range) = diconfig.update_status(status);
			*diconfig = new_diconfig;
			range
		};

		if let Some(range) = range {
			for row in range {
				self.notify.row_changed(row);
			}
		} else {
			self.notify.reset();
		}
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
					.map(|opt| match (opt.name, opt.description) {
						(None, _) => self.none_string.clone(),
						(Some(name), None) => SharedString::from(name.as_ref()),
						(Some(name), Some(desc)) => format!("{desc} ({name})").into(),
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
		let options = VecModel::from(options);
		let options = ModelRc::new(options);

		let result = DeviceAndImageEntry {
			display_tag,
			indent,
			options,
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
