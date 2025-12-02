use std::cell::RefCell;
use std::collections::HashMap;
use std::iter::once;
use std::rc::Rc;

use anyhow::Error;
use itertools::Itertools;
use showfile::show_path_in_file_manager;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Global;
use slint::Model;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;
use slint::spawn_local;
use smol_str::SmolStr;
use tokio::sync::mpsc;

use crate::action::Action;
use crate::devimageconfig::DevicesImagesConfig;
use crate::devimageconfig::EntryDetails;
use crate::devimageconfig::ListSlots;
use crate::dialogs::SenderExt;
use crate::dialogs::devimages::entry_popup_menu;
use crate::dialogs::image::Format;
use crate::dialogs::image::dialog_load_image;
use crate::dialogs::socket::dialog_connect_to_socket;
use crate::guiutils::modal::ModalStack;
use crate::imagedesc::ImageDesc;
use crate::info::InfoDb;
use crate::info::Machine;
use crate::info::View;
use crate::mconfig::MachineConfig;
use crate::models::audit::AuditModel;
use crate::models::devimages::DevicesAndImagesModel;
use crate::prefs::PrefsItem;
use crate::prefs::PrefsMachineItem;
use crate::prefs::PrefsPaths;
use crate::prefs::PrefsSoftwareItem;
use crate::software::SoftwareListDispenser;
use crate::ui::ConfigureDialog;
use crate::ui::DeviceAndImageEntry;
use crate::ui::DevicesAndImagesState;
use crate::ui::Icons;
use crate::ui::SoftwareMachine;

struct State {
	dialog_weak: Weak<ConfigureDialog>,
	modal_stack: ModalStack,
	core: CoreState,
}

enum CoreState {
	Machine {
		dimodel_state: DiModelState,
		ram_size: Option<u64>,
		bios: Option<String>,
	},
	Software {
		info_db: Rc<InfoDb>,
		software_list: String,
		software: String,
		software_machines: ModelRc<SoftwareMachine>,
	},
}

enum DiModelState {
	Ok {
		dimodel: ModelRc<DeviceAndImageEntry>,
		images: RefCell<HashMap<String, ImageDesc>>,
	},
	Error {
		error: Error,
		info_db: Rc<InfoDb>,
		machine_index: usize,
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

	// do we have a devices and images model?
	if let Some(dimodel) = state.dimodel() {
		// if so we have lots to set up
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
			let CoreState::Machine { dimodel_state, .. } = &state_clone.core else {
				unreachable!()
			};
			let DiModelState::Ok { dimodel, .. } = dimodel_state else {
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
		modal.dialog().on_menu_item_action(move |command_string| {
			if let Some(command) = Action::decode_from_slint(command_string) {
				context_menu_command(&state_clone, command);
			}
		});
	}

	// RAM options
	if let Some((ram_option_texts, current_index)) = state.ram_options() {
		let ram_option_texts = VecModel::from(ram_option_texts);
		let ram_option_texts = ModelRc::new(ram_option_texts);
		modal.dialog().set_ram_sizes_model(ram_option_texts);

		// workaround for https://github.com/slint-ui/slint/issues/7632; please remove hack when fixed
		let dialog_weak = modal.dialog().as_weak();
		let fut = async move {
			tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
			let current_index = current_index.try_into().unwrap();
			dialog_weak.unwrap().set_ram_sizes_index(current_index);
		};
		spawn_local(fut).unwrap();
	}

	// BIOS options
	if let Some((bios_option_texts, current_index)) = state.bios_options() {
		let bios_option_texts = VecModel::from(bios_option_texts);
		let bios_option_texts = ModelRc::new(bios_option_texts);
		modal.dialog().set_bios_selection_model(bios_option_texts);

		// workaround for https://github.com/slint-ui/slint/issues/7632; please remove hack when fixed
		let dialog_weak = modal.dialog().as_weak();
		let fut = async move {
			tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
			let current_index = current_index.try_into().unwrap();
			dialog_weak.unwrap().set_bios_selection_index(current_index);
		};
		spawn_local(fut).unwrap();
	}

	// Asset audit
	if let CoreState::Machine { dimodel_state, .. } = &state.core {
		dimodel_state.with_machine(|machine| {
			if let Some(machine) = machine {
				let icons = Icons::get(modal.dialog());
				let rom_paths = paths.roms.clone().into();
				let sample_paths = paths.samples.clone().into();
				let model = AuditModel::new(machine, rom_paths, sample_paths, icons);
				let model = ModelRc::new(model);
				modal.dialog().set_audit_assets(model.clone());

				// lambda to run the audit
				let run_audit = move || {
					let model = model.clone();
					let fut = async move {
						let model = AuditModel::get_model(&model);
						model.run_audit().await;
					};
					spawn_local(fut).unwrap();
				};

				// set up the click handler
				modal.dialog().on_run_audit_clicked(run_audit.clone());

				// and run an audit now!
				run_audit();
			}
		});
	}

	// loading error?
	if let Some(error) = state.error() {
		let text = error.to_string().into();
		modal.dialog().set_dev_images_error(text);
	}

	// software machines?
	if let Some(software_machines) = state.software_machines() {
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

fn context_menu_command(state: &Rc<State>, command: Action) {
	// this dialog interprets commands differently than core BletchMAME
	match command {
		Action::LoadImageDialog { tag } => {
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
		Action::ConnectToSocketDialog { tag } => {
			let state = state.clone();
			let fut = async move {
				if let Some(image_desc) = dialog_connect_to_socket(state.modal_stack.clone()).await {
					let image_desc = Some(image_desc);
					state.set_image_imagedesc(tag, image_desc);
				}
			};
			spawn_local(fut).unwrap();
		}
		Action::UnloadImage { tag } => {
			state.set_image_imagedesc(tag, None);
		}
		Action::Launch(path) => {
			let _ = open::that(path);
		}
		Action::ShowFile(path) => {
			show_path_in_file_manager(&path);
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
				let bios = item.bios.clone();
				let dimodel_state = match machine_config {
					Ok(machine_config) => {
						let diconfig = DevicesImagesConfig::from(machine_config);
						let dimodel = DevicesAndImagesModel::new(diconfig);
						let dimodel: ModelRc<DeviceAndImageEntry> = ModelRc::new(dimodel);
						let images = RefCell::new(item.images);
						DiModelState::Ok { dimodel, images }
					}
					Err(error) => DiModelState::Error {
						info_db: info_db.clone(),
						machine_index,
						error,
					},
				};
				CoreState::Machine {
					dimodel_state,
					ram_size,
					bios,
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

	pub fn dimodel(&self) -> Option<&'_ ModelRc<DeviceAndImageEntry>> {
		if let CoreState::Machine {
			dimodel_state: DiModelState::Ok { dimodel, .. },
			..
		} = &self.core
		{
			Some(dimodel)
		} else {
			None
		}
	}

	pub fn ram_options(&self) -> Option<(Vec<SharedString>, usize)> {
		let CoreState::Machine {
			dimodel_state,
			ram_size,
			..
		} = &self.core
		else {
			return None;
		};
		dimodel_state.with_machine(|machine| {
			let machine = machine.unwrap();
			let ram_options = machine.ram_options();
			(!ram_options.is_empty()).then(|| {
				// figure out the default RAM option
				let default_index = machine
					.default_ram_option_index()
					.expect("expected a default RAM option");
				let default_text = ram_size_display_text(ram_options.get(default_index).unwrap().size());
				let default_text = default_text_string(&default_text);

				// build the text for the RAM options
				let ram_option_texts = once(SharedString::from(default_text))
					.chain(ram_options.iter().map(|opt| ram_size_display_text(opt.size()).into()))
					.collect::<Vec<_>>();

				// current RAM size option
				let current_index = ram_size
					.and_then(|ram_size| ram_options.iter().position(|x| x.size() == ram_size))
					.map(|idx| idx + 1)
					.unwrap_or_default();

				(ram_option_texts, current_index)
			})
		})
	}

	pub fn bios_options(&self) -> Option<(Vec<SharedString>, usize)> {
		let CoreState::Machine {
			dimodel_state, bios, ..
		} = &self.core
		else {
			return None;
		};
		dimodel_state.with_machine(|machine| {
			let machine = machine.unwrap();
			let biossets = machine.biossets();
			(!biossets.is_empty()).then(|| {
				// figure out the default BIOS
				let default_index = machine
					.default_biosset_index()
					.expect("expected a default BIOS set index");
				let default_text = default_text_string(biossets.get(default_index).unwrap().description());

				// build the text for the BIOS options
				let bios_option_texts = once(SharedString::from(default_text))
					.chain(biossets.iter().map(|opt| opt.description().into()))
					.collect::<Vec<_>>();

				// current BIOS option
				let current_index = bios
					.as_deref()
					.and_then(|bios| biossets.iter().position(|x| x.name() == bios))
					.map(|idx| idx + 1)
					.unwrap_or_default();

				(bios_option_texts, current_index)
			})
		})
	}

	pub fn error(&self) -> Option<&Error> {
		if let CoreState::Machine {
			dimodel_state: DiModelState::Error { error, .. },
			..
		} = &self.core
		{
			Some(error)
		} else {
			None
		}
	}

	pub fn software_machines(&self) -> Option<&ModelRc<SoftwareMachine>> {
		if let CoreState::Software { software_machines, .. } = &self.core {
			Some(software_machines)
		} else {
			None
		}
	}

	pub fn with_diconfig<R>(&self, callback: impl FnOnce(&DevicesImagesConfig) -> R) -> R {
		let CoreState::Machine {
			dimodel_state: DiModelState::Ok { dimodel, .. },
			..
		} = &self.core
		else {
			unreachable!()
		};
		let dimodel = DevicesAndImagesModel::get_model(dimodel);
		dimodel.with_diconfig(callback)
	}

	pub fn update_images(&self) {
		let CoreState::Machine {
			dimodel_state: DiModelState::Ok { dimodel, images },
			..
		} = &self.core
		else {
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
				let slots = diconfig.list_slots(ListSlots::NonDefault);
				let slots = slots
					.into_iter()
					.map(|(slot, option_name)| (slot.to_string(), option_name.map(str::to_string)))
					.collect::<Vec<_>>();
				let images = diconfig
					.images()
					.filter_map(|(tag, filename)| filename.map(|image_desc| (tag.to_string(), image_desc.clone())))
					.collect::<HashMap<_, _>>();
				let ram_sizes_index = dialog.get_ram_sizes_index();
				let ram_size = usize::try_from(ram_sizes_index - 1)
					.ok()
					.map(|index| machine.ram_options().get(index).unwrap().size());
				let bios_index = dialog.get_bios_selection_index();
				let bios = usize::try_from(bios_index - 1)
					.ok()
					.map(|index| machine.biossets().get(index).unwrap().name().into());
				let item = PrefsMachineItem {
					machine_name,
					slots,
					images,
					ram_size,
					bios,
				};
				PrefsItem::Machine(item)
			}),

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
		let CoreState::Machine {
			dimodel_state: DiModelState::Ok { dimodel, .. },
			..
		} = &self.core
		else {
			unreachable!()
		};
		let dimodel = DevicesAndImagesModel::get_model(dimodel);
		dimodel.set_slot_entry_option(entry_index, new_option_name);
		self.update_images();
	}

	pub fn set_image_imagedesc(&self, tag: String, image_desc: Option<ImageDesc>) {
		let CoreState::Machine {
			dimodel_state: DiModelState::Ok { images, .. },
			..
		} = &self.core
		else {
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
			CoreState::Machine { dimodel_state, .. } => {
				if let DiModelState::Ok { dimodel, .. } = dimodel_state {
					let dimodel = DevicesAndImagesModel::get_model(dimodel);
					dimodel.change_diconfig(|diconfig| Some(diconfig.reset_to_defaults()));
				}
			}
			CoreState::Software { .. } => todo!(),
		}
	}

	pub fn title(&self, paths: &PrefsPaths) -> String {
		match &self.core {
			CoreState::Machine { dimodel_state, .. } => dimodel_state.with_machine(|machine| {
				let machine = machine.unwrap();
				configure_dialog_title(machine.description(), Some(machine.name()))
			}),

			CoreState::Software {
				info_db,
				software_list,
				software,
				..
			} => {
				let mut dispenser = SoftwareListDispenser::new(info_db, &paths.software_lists);

				let software_entry = dispenser.get(software_list).ok().and_then(|(_, x)| {
					x.software
						.iter()
						.flat_map(|x| (x.name == software).then(|| x.clone()))
						.next()
				});
				if let Some(software_entry) = software_entry.as_deref() {
					configure_dialog_title(software_entry.description.as_ref(), Some(software_entry.name.as_ref()))
				} else {
					configure_dialog_title(software.as_str(), None)
				}
			}
		}
	}
}

impl DiModelState {
	pub fn with_machine<R>(&self, callback: impl FnOnce(Option<Machine<'_>>) -> R) -> R {
		match self {
			Self::Ok { dimodel, .. } => {
				let dimodel = DevicesAndImagesModel::get_model(dimodel);
				dimodel.with_diconfig(|diconfig| callback(diconfig.machine()))
			}
			Self::Error {
				info_db, machine_index, ..
			} => {
				let machine = info_db.machines().get(*machine_index).unwrap();
				callback(Some(machine))
			}
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

fn ram_size_display_text(ram_size: u64) -> String {
	let ram_size = byte_unit::Byte::from_u64(ram_size);
	let (n, unit) = ram_size.get_exact_unit(true);
	format!("{n} {unit}")
}

fn default_text_string(s: &str) -> String {
	format!("Default ({s})")
}
