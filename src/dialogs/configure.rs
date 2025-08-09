use std::cell::RefCell;
use std::collections::HashMap;
use std::iter::once;
use std::rc::Rc;

use anyhow::Error;
use itertools::Itertools;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Model;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;
use slint::spawn_local;
use smol_str::SmolStr;
use tokio::sync::mpsc;

use crate::appcommand::AppCommand;
use crate::devimageconfig::DevicesImagesConfig;
use crate::devimageconfig::EntryDetails;
use crate::dialogs::SenderExt;
use crate::dialogs::devimages::entry_popup_menu;
use crate::dialogs::image::Format;
use crate::dialogs::image::dialog_load_image;
use crate::dialogs::socket::dialog_connect_to_socket;
use crate::guiutils::modal::ModalStack;
use crate::imagedesc::ImageDesc;
use crate::info::InfoDb;
use crate::info::View;
use crate::mconfig::MachineConfig;
use crate::models::devimages::DevicesAndImagesModel;
use crate::prefs::PrefsItem;
use crate::prefs::PrefsMachineItem;
use crate::prefs::PrefsPaths;
use crate::prefs::PrefsSoftwareItem;
use crate::software::SoftwareListDispenser;
use crate::ui::ConfigureDialog;
use crate::ui::DeviceAndImageEntry;
use crate::ui::DevicesAndImagesState;
use crate::ui::SoftwareMachine;

struct State {
	dialog_weak: Weak<ConfigureDialog>,
	modal_stack: ModalStack,
	core: CoreState,
}

enum CoreState {
	Machine {
		dimodel: ModelRc<DeviceAndImageEntry>,
		images: RefCell<HashMap<String, ImageDesc>>,
		ram_size: Option<u64>,
	},
	MachineError {
		info_db: Rc<InfoDb>,
		machine_index: usize,
		ram_size: Option<u64>,
		error: Error,
	},
	Software {
		info_db: Rc<InfoDb>,
		software_list: String,
		software: String,
		software_machines: ModelRc<SoftwareMachine>,
	},
}

pub async fn dialog_configure(
	modal_stack: ModalStack,
	info_db: Rc<InfoDb>,
	item: PrefsItem,
	paths: &PrefsPaths,
) -> Option<PrefsItem> {
	// prepare the dialog
	let modal = modal_stack.modal(|| ConfigureDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// get the state
	let dialog_weak = modal.dialog().as_weak();
	let modal_stack = modal_stack.clone();
	let state: State = State::new(dialog_weak, modal_stack, &info_db, item);
	let state = Rc::new(state);

	// set the title
	modal.dialog().set_dialog_title(state.title(paths).into());

	// do different things based on the state
	let ram_info = match &state.core {
		CoreState::Machine { dimodel, ram_size, .. } => {
			// set up the devices and images model
			let none_string = DevicesAndImagesModel::get_model(dimodel).none_string.clone();
			let distate = DevicesAndImagesState {
				entries: dimodel.clone(),
				none_string,
			};
			modal.dialog().set_dev_images_state(distate);

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
				let CoreState::Machine { dimodel, .. } = &state_clone.core else {
					unreachable!()
				};
				let model = DevicesAndImagesModel::get_model(dimodel);
				let entry_index = entry_index.try_into().unwrap();
				entry_popup_menu(model, entry_index, point, |entries, point| {
					dialog.invoke_show_context_menu(entries, point)
				})
			});

			// set up the context menu command handler
			let state_clone = state.clone();
			modal.dialog().on_menu_item_command(move |command_string| {
				if let Some(command) = AppCommand::decode_from_slint(command_string) {
					context_menu_command(&state_clone, command);
				}
			});

			// RAM info
			let machine_index = state.with_diconfig(|diconfig| diconfig.machine().unwrap().index());
			Some((machine_index, *ram_size))
		}

		CoreState::MachineError {
			error,
			machine_index,
			ram_size,
			..
		} => {
			let text = error.to_string().into();
			modal.dialog().set_dev_images_error(text);

			// RAM info
			Some((*machine_index, *ram_size))
		}

		CoreState::Software { software_machines, .. } => {
			modal.dialog().set_software_machines(software_machines.clone());
			state.update_software_machines_bulk_enabled();

			let state_clone = state.clone();
			modal.dialog().on_software_machines_toggle_checked(move |index| {
				state_clone.toggle_software_machines_checked(index.try_into().unwrap())
			});
			let state_clone = state.clone();
			modal
				.dialog()
				.on_software_machines_bulk_none_clicked(move || state_clone.set_software_machines_bulk_checked(false));
			let state_clone = state.clone();
			modal
				.dialog()
				.on_software_machines_bulk_all_clicked(move || state_clone.set_software_machines_bulk_checked(true));

			None
		}
	};

	// set up RAM size options
	if let Some((machine_index, ram_size)) = ram_info {
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
			let index = ram_size
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
	}

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// set up the "ok" button
	let tx_clone = tx.clone();
	let state_clone = state.clone();
	modal.dialog().on_ok_clicked(move || {
		let result = state_clone.get_prefs_item();
		tx_clone.signal(Some(result));
	});

	// set up the "cancel" button
	let tx_clone = tx.clone();
	modal.dialog().on_cancel_clicked(move || {
		tx_clone.signal(None);
	});

	// set up the "reset" button
	let state_clone = state.clone();
	modal.dialog().on_reset_clicked(move || {
		state_clone.reset_to_defaults();
	});

	// present the modal dialog
	modal.run(async { rx.recv().await.unwrap() }).await
}

fn ram_size_display_text(ram_size: u64) -> String {
	let ram_size = byte_unit::Byte::from_u64(ram_size);
	let (n, unit) = ram_size.get_exact_unit(true);
	format!("{n} {unit}")
}

fn context_menu_command(state: &Rc<State>, command: AppCommand) {
	// this dialog interprets commands differently than core BletchMAME
	match command {
		AppCommand::LoadImageDialog { tag } => {
			let (_image_desc, extensions) = state.with_diconfig(|config| {
				let image_desc = (0..config.entry_count())
					.filter_map(|index| {
						let entry = config.entry(index).unwrap();
						let EntryDetails::Image { image_desc } = &entry.details else {
							return None;
						};
						(entry.tag == tag).then(|| image_desc.cloned())
					})
					.next()
					.unwrap();

				let (_, device) = config.machine_config().unwrap().lookup_device_tag(&tag).unwrap();
				let extensions = device.extensions().map(SmolStr::from).collect::<Vec<_>>();
				(image_desc, extensions)
			});

			let state = state.clone();
			let fut = async move {
				let formats = [Format {
					description: "Image File".into(),
					extensions,
				}];
				if let Some(image_desc) = dialog_load_image(state.modal_stack.clone(), &formats).await {
					let image_desc = Some(image_desc);
					state.set_image_imagedesc(tag, image_desc);
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::ConnectToSocketDialog { tag } => {
			let state = state.clone();
			let fut = async move {
				if let Some(image_desc) = dialog_connect_to_socket(state.modal_stack.clone()).await {
					let image_desc = Some(image_desc);
					state.set_image_imagedesc(tag, image_desc);
				}
			};
			spawn_local(fut).unwrap();
		}
		AppCommand::UnloadImage { tag } => {
			state.set_image_imagedesc(tag, None);
		}
		_ => unreachable!(),
	}
}

impl State {
	pub fn new(
		dialog_weak: Weak<ConfigureDialog>,
		modal_stack: ModalStack,
		info_db: &Rc<InfoDb>,
		item: PrefsItem,
	) -> Self {
		let core = match item {
			PrefsItem::Machine(item) => {
				// figure out the diconfig
				let machine_index = info_db.machines().find_index(&item.machine_name).unwrap();
				let machine_config =
					MachineConfig::from_machine_index_and_slots(info_db.clone(), machine_index, &item.slots);
				let ram_size = item.ram_size;
				match machine_config {
					Ok(machine_config) => {
						let diconfig = DevicesImagesConfig::from(machine_config);
						let dimodel = DevicesAndImagesModel::new(diconfig);
						let dimodel: ModelRc<DeviceAndImageEntry> = ModelRc::new(dimodel);
						let images = RefCell::new(item.images);
						CoreState::Machine {
							dimodel,
							images,
							ram_size,
						}
					}
					Err(error) => CoreState::MachineError {
						info_db: info_db.clone(),
						machine_index,
						ram_size,
						error,
					},
				}
			}
			PrefsItem::Software(item) => {
				let software_list = info_db.software_lists().find(&item.software_list).unwrap();
				let software_machines = software_list
					.original_for_machines()
					.iter()
					.chain(software_list.compatible_for_machines().iter())
					.sorted_by_key(|machine| machine.description())
					.map(|machine| {
						let machine_index = machine.index().try_into().unwrap();
						let checked = item
							.preferred_machines
							.as_ref()
							.is_none_or(|x| x.iter().any(|x| x == machine.name()));
						let description = machine.description().into();
						SoftwareMachine {
							machine_index,
							description,
							checked,
						}
					})
					.collect::<Vec<_>>();
				let software_machines = VecModel::from(software_machines);
				let software_machines = ModelRc::new(software_machines);

				CoreState::Software {
					info_db: info_db.clone(),
					software_list: item.software_list,
					software: item.software,
					software_machines,
				}
			}
		};
		let state = Self {
			dialog_weak,
			modal_stack,
			core,
		};
		if matches!(&state.core, CoreState::Machine { .. }) {
			state.update_images();
		}
		state
	}

	pub fn with_diconfig<R>(&self, callback: impl FnOnce(&DevicesImagesConfig) -> R) -> R {
		let CoreState::Machine { dimodel, .. } = &self.core else {
			unreachable!()
		};
		let dimodel = DevicesAndImagesModel::get_model(dimodel);
		dimodel.with_diconfig(callback)
	}

	pub fn update_images(&self) {
		let CoreState::Machine { dimodel, images, .. } = &self.core else {
			unreachable!()
		};
		let dimodel = DevicesAndImagesModel::get_model(dimodel);
		let images = images.borrow();
		dimodel.change_diconfig(|diconfig| {
			let diconfig = diconfig.set_images_from_slots(|tag| images.get(tag).cloned());
			Some(diconfig)
		});
	}

	pub fn get_prefs_item(&self) -> PrefsItem {
		let dialog = self.dialog_weak.unwrap();

		match &self.core {
			CoreState::Machine { .. } => self.with_diconfig(|diconfig| {
				let machine = diconfig.machine().unwrap();
				let machine_name = machine.name().to_string();
				let slots = diconfig.changed_slots(false);
				let images = diconfig
					.images()
					.filter_map(|(tag, filename)| filename.map(|image_desc| (tag.to_string(), image_desc.clone())))
					.collect::<HashMap<_, _>>();
				let ram_sizes_index = dialog.get_ram_sizes_index();
				let ram_size = usize::try_from(ram_sizes_index - 1)
					.ok()
					.map(|index| machine.ram_options().get(index).unwrap().size());
				let item = PrefsMachineItem {
					machine_name,
					slots,
					images,
					ram_size,
				};
				PrefsItem::Machine(item)
			}),

			CoreState::MachineError { .. } => todo!(),

			CoreState::Software {
				info_db,
				software_list,
				software,
				software_machines,
			} => {
				let software_machines = software_machines
					.as_any()
					.downcast_ref::<VecModel<SoftwareMachine>>()
					.unwrap();
				let software_machines_len = software_machines.row_count();
				let preferred_machine_indexes = software_machines
					.iter()
					.filter_map(|x| x.checked.then_some(x.machine_index))
					.collect::<Vec<_>>();
				let preferred_machines = (preferred_machine_indexes.len() != software_machines_len).then(|| {
					preferred_machine_indexes
						.into_iter()
						.map(|machine_index| {
							let machine_index = machine_index.try_into().unwrap();
							info_db.machines().get(machine_index).unwrap().name().to_string()
						})
						.collect::<Vec<_>>()
				});

				let item = PrefsSoftwareItem {
					software_list: software_list.clone(),
					software: software.clone(),
					preferred_machines,
				};
				PrefsItem::Software(item)
			}
		}
	}

	pub fn set_slot_entry_option(&self, entry_index: usize, new_option_name: Option<&str>) {
		let CoreState::Machine { dimodel, .. } = &self.core else {
			unreachable!()
		};
		let dimodel = DevicesAndImagesModel::get_model(dimodel);
		dimodel.set_slot_entry_option(entry_index, new_option_name);
		self.update_images();
	}

	pub fn set_image_imagedesc(&self, tag: String, image_desc: Option<ImageDesc>) {
		let CoreState::Machine { images, .. } = &self.core else {
			unreachable!()
		};
		if let Some(image_desc) = image_desc {
			images.borrow_mut().insert(tag, image_desc);
		} else {
			images.borrow_mut().remove(&tag);
		}
		self.update_images();
	}

	pub fn update_software_machines_bulk_enabled(&self) {
		let CoreState::Software { software_machines, .. } = &self.core else {
			unreachable!()
		};
		let all_equal_value = software_machines
			.as_any()
			.downcast_ref::<VecModel<SoftwareMachine>>()
			.unwrap()
			.iter()
			.map(|x| x.checked)
			.all_equal_value();
		let (bulk_all_enabled, bulk_none_enabled) = match all_equal_value {
			Ok(false) => (true, false),
			Ok(true) => (false, true),
			Err(None) => (false, false),
			Err(Some(_)) => (true, true),
		};
		let dialog = self.dialog_weak.unwrap();
		dialog.set_software_machines_bulk_all_enabled(bulk_all_enabled);
		dialog.set_software_machines_bulk_none_enabled(bulk_none_enabled);
	}

	pub fn toggle_software_machines_checked(&self, row: usize) {
		let CoreState::Software { software_machines, .. } = &self.core else {
			unreachable!()
		};
		let model = software_machines
			.as_any()
			.downcast_ref::<VecModel<SoftwareMachine>>()
			.unwrap();
		let mut data = model.row_data(row).unwrap();
		data.checked = !data.checked;
		model.set_row_data(row, data);
		self.update_software_machines_bulk_enabled();
	}

	pub fn set_software_machines_bulk_checked(&self, checked: bool) {
		let CoreState::Software { software_machines, .. } = &self.core else {
			unreachable!()
		};
		let model = software_machines
			.as_any()
			.downcast_ref::<VecModel<SoftwareMachine>>()
			.unwrap();

		for (row, data) in model.iter().enumerate() {
			if data.checked != checked {
				let data = SoftwareMachine { checked, ..data };
				model.set_row_data(row, data);
			}
		}

		self.update_software_machines_bulk_enabled();
	}

	pub fn reset_to_defaults(&self) {
		let dialog = self.dialog_weak.unwrap();
		dialog.set_ram_sizes_index(0);

		match &self.core {
			CoreState::Machine { dimodel, .. } => {
				let dimodel = DevicesAndImagesModel::get_model(dimodel);
				dimodel.change_diconfig(|diconfig| Some(diconfig.reset_to_defaults()));
			}
			CoreState::MachineError { .. } => {}
			CoreState::Software { .. } => todo!(),
		}
	}

	pub fn title(&self, paths: &PrefsPaths) -> String {
		if let CoreState::Software {
			info_db,
			software_list,
			software,
			..
		} = &self.core
		{
			let mut dispenser = SoftwareListDispenser::new(info_db, &paths.software_lists);

			let software_entry = dispenser.get(software_list).ok().and_then(|(_, x)| {
				x.software
					.iter()
					.flat_map(|x| (x.name.as_ref() == software).then(|| x.clone()))
					.next()
			});
			if let Some(software_entry) = software_entry.as_deref() {
				configure_dialog_title(software_entry.description.as_ref(), Some(software_entry.name.as_ref()))
			} else {
				configure_dialog_title(software.as_str(), None)
			}
		} else {
			let (info_db, machine_index) = match &self.core {
				CoreState::Machine { dimodel, .. } => {
					DevicesAndImagesModel::get_model(dimodel).with_diconfig(|diconfig| {
						(
							diconfig.machine_config().unwrap().info_db.clone(),
							diconfig.machine().unwrap().index(),
						)
					})
				}
				CoreState::MachineError {
					info_db, machine_index, ..
				} => (info_db.clone(), *machine_index),
				CoreState::Software { .. } => unreachable!(),
			};
			let machine = info_db.machines().get(machine_index).unwrap();
			configure_dialog_title(machine.description(), Some(machine.name()))
		}
	}
}

fn configure_dialog_title(description: &str, name: Option<&str>) -> String {
	if let Some(name) = name {
		format!("Configure {description} ({name})")
	} else {
		format!("Configure {description}")
	}
}
