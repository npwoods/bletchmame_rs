use std::cell::RefCell;
use std::collections::HashMap;
use std::iter::once;
use std::rc::Rc;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;
use slint::spawn_local;

use crate::appcommand::AppCommand;
use crate::devimageconfig::DevicesImagesConfig;
use crate::devimageconfig::EntryDetails;
use crate::dialogs::SingleResult;
use crate::dialogs::devimages::entry_popup_menu;
use crate::dialogs::image::Format;
use crate::dialogs::image::dialog_load_image;
use crate::guiutils::MenuingType;
use crate::guiutils::modal::Modal;
use crate::info::InfoDb;
use crate::info::View;
use crate::mconfig::MachineConfig;
use crate::models::devimages::DevicesAndImagesModel;
use crate::prefs::PrefsMachineItem;
use crate::ui::ConfigureDialog;
use crate::ui::DeviceAndImageEntry;
use crate::ui::DevicesAndImagesState;

struct State {
	dialog_weak: Weak<ConfigureDialog>,
	dimodel: ModelRc<DeviceAndImageEntry>,
	images: RefCell<HashMap<String, String>>,
}

pub async fn dialog_configure(
	parent: Weak<impl ComponentHandle + 'static>,
	info_db: Rc<InfoDb>,
	item: PrefsMachineItem,
	menuing_type: MenuingType,
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
			let dimodel = DevicesAndImagesModel::new(diconfig);
			let none_string = dimodel.none_string.clone();
			let dimodel: ModelRc<DeviceAndImageEntry> = ModelRc::new(dimodel);
			let distate = DevicesAndImagesState {
				entries: dimodel.clone(),
				none_string,
			};
			modal.dialog().set_dev_images_state(distate);

			// assemble what we have into dialog state
			let dialog_weak = modal.dialog().as_weak();
			let images = RefCell::new(item.images);
			let state = State {
				dialog_weak,
				dimodel,
				images,
			};
			let state = Rc::new(state);

			// initial images setup
			state.update_images();

			// set up the "ok" button
			let signaller = single_result.signaller();
			let state_clone = state.clone();
			modal.dialog().on_ok_clicked(move || {
				let result = state_clone.get_machine_item();
				signaller.signal(Some(result));
			});

			// set up callback for when an entry option changed
			let state_clone = state.clone();
			modal
				.dialog()
				.on_entry_option_changed(move |entry_index, new_option_name| {
					let entry_index = entry_index.try_into().unwrap();
					let new_option_name = (!new_option_name.is_empty()).then_some(new_option_name.as_str());
					state_clone.set_slot_entry_option(entry_index, new_option_name);
				});

			// set up callback for when an image button is pressed
			let state_clone = state.clone();
			modal.dialog().on_entry_button_clicked(move |entry_index, point| {
				let dialog = state_clone.dialog_weak.unwrap();
				let model = DevicesAndImagesModel::get_model(&state_clone.dimodel);
				let entry_index = entry_index.try_into().unwrap();
				entry_popup_menu(
					dialog.window(),
					model,
					menuing_type,
					entry_index,
					point,
					|entries, point| dialog.invoke_show_context_menu(entries, point),
				)
			});

			// set up a command filter
			let state_clone = state.clone();
			modal.set_command_filter(move |command| command_filter(&state_clone, command));
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

		// workaround for https://github.com/slint-ui/slint/issues/7632; please remove hack when fixed
		let dialog_weak = modal.dialog().as_weak();
		let fut = async move {
			tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
			dialog_weak.unwrap().set_ram_sizes_index(index.try_into().unwrap());
		};
		spawn_local(fut).unwrap();
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

fn ram_size_display_text(ram_size: u64) -> String {
	let ram_size = byte_unit::Byte::from_u64(ram_size);
	let (n, unit) = ram_size.get_exact_unit(true);
	format!("{n} {unit}")
}

fn command_filter(state: &Rc<State>, command: AppCommand) -> Option<AppCommand> {
	// first pass
	match command {
		AppCommand::LoadImageDialog { tag } => {
			let (_filename, extensions) = state.with_diconfig(|config| {
				let filename = (0..config.entry_count())
					.filter_map(|index| {
						let entry = config.entry(index).unwrap();
						let EntryDetails::Image { filename } = &entry.details else {
							return None;
						};
						(entry.tag == tag).then(|| filename.map(|x| x.to_string()))
					})
					.next()
					.unwrap();

				let (_, device) = config.machine_config().unwrap().lookup_device_tag(&tag).unwrap();
				let extensions = device.extensions().map(str::to_string).collect::<Vec<_>>();
				(filename, extensions)
			});

			let formats = [Format {
				description: "Image File",
				extensions: &extensions,
			}];
			let format_iter = formats.iter().cloned();
			dialog_load_image(state.dialog_weak.clone(), format_iter)
				.and_then(|filename| command_filter(state, AppCommand::LoadImage { tag, filename }))
		}
		AppCommand::LoadImage { tag, filename } => {
			state.set_image_filename(tag, Some(filename));
			None
		}
		AppCommand::UnloadImage { tag } => {
			state.set_image_filename(tag, None);
			None
		}
		_ => Some(command),
	}
}

impl State {
	pub fn with_diconfig<R>(&self, callback: impl FnOnce(&DevicesImagesConfig) -> R) -> R {
		let dimodel = DevicesAndImagesModel::get_model(&self.dimodel);
		dimodel.with_diconfig(callback)
	}

	pub fn update_images(&self) {
		let dimodel = DevicesAndImagesModel::get_model(&self.dimodel);
		let images = self.images.borrow();
		dimodel.change_diconfig(|diconfig| {
			let diconfig = diconfig.set_images_from_slots(|tag| images.get(tag).cloned());
			Some(diconfig)
		});
	}

	pub fn get_machine_item(&self) -> PrefsMachineItem {
		let dialog = self.dialog_weak.unwrap();
		self.with_diconfig(|diconfig| {
			let machine = diconfig.machine().unwrap();
			let machine_name = machine.name().to_string();
			let slots = diconfig.changed_slots(false);
			let images = diconfig
				.images()
				.filter_map(|(tag, filename)| filename.map(|filename| (tag.to_string(), filename.to_string())))
				.collect::<HashMap<_, _>>();
			let ram_sizes_index = dialog.get_ram_sizes_index();
			let ram_size = usize::try_from(ram_sizes_index - 1)
				.ok()
				.map(|index| machine.ram_options().get(index).unwrap().size());
			PrefsMachineItem {
				machine_name,
				slots,
				images,
				ram_size,
			}
		})
	}

	pub fn set_slot_entry_option(&self, entry_index: usize, new_option_name: Option<&str>) {
		let dimodel = DevicesAndImagesModel::get_model(&self.dimodel);
		dimodel.set_slot_entry_option(entry_index, new_option_name);
		self.update_images();
	}

	pub fn set_image_filename(&self, tag: String, new_filename: Option<String>) {
		if let Some(new_filename) = new_filename {
			self.images.borrow_mut().insert(tag, new_filename);
		} else {
			self.images.borrow_mut().remove(&tag);
		}
		self.update_images();
	}
}
