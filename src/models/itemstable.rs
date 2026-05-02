use std::any::Any;
use std::borrow::Cow;
use std::cell::Cell;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::iter::once;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Error;
use anyhow::Result;
use itertools::Either;
use itertools::Itertools;
use levenshtein::levenshtein;
use nu_utils::IgnoreCaseExt;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::SharedString;
use slint::StandardListViewItem;
use slint::ToSharedString;
use slint::VecModel;
use smallvec::SmallVec;
use smol_str::SmolStr;
use tracing::debug;
use tracing::debug_span;
use tracing::info_span;
use unicase::UniCase;

use crate::action::Action;
use crate::imagedesc::ImageDesc;
use crate::info::InfoDb;
use crate::info::View;
use crate::mconfig::MachineConfig;
use crate::prefs::BuiltinCollection;
use crate::prefs::ColumnType;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsItem;
use crate::prefs::PrefsItemDetails;
use crate::prefs::PrefsItemRef;
use crate::prefs::PrefsMachineItem;
use crate::prefs::PrefsSoftwareItem;
use crate::prefs::SortOrder;
use crate::runtime::MameStartArgs;
use crate::selection::SelectionManager;
use crate::software::Software;
use crate::software::SoftwareList;
use crate::software::SoftwareListDispenser;
use crate::ui::ItemContextMenuInfo;
use crate::ui::SimpleMenuEntry;

pub struct ItemsTableModel {
	info_db: RefCell<Option<Rc<InfoDb>>>,
	software_list_paths: RefCell<Vec<SmolStr>>,
	column_types: RefCell<Rc<[ColumnType]>>,
	sorting: Cell<Option<(ColumnType, SortOrder)>>,
	search: RefCell<String>,
	items: RefCell<Rc<[Item]>>,
	items_map: RefCell<Box<[u32]>>,

	current_collection: RefCell<Option<Rc<PrefsCollection>>>,
	selected_index: Cell<Option<u32>>,

	selection: SelectionManager,
	empty_callback: Box<dyn Fn(Option<EmptyReason>) + 'static>,
	notify: ModelNotify,
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("unknown software {0}")]
	UnknownSoftware(String),
}

impl ItemsTableModel {
	pub fn new(selection: SelectionManager, empty_callback: impl Fn(Option<EmptyReason>) + 'static) -> Rc<Self> {
		let result = Self {
			info_db: RefCell::new(None),
			software_list_paths: RefCell::new([].into()),
			column_types: RefCell::new([].into()),
			sorting: Cell::new(None),
			search: RefCell::new("".into()),
			items: RefCell::new([].into()),
			items_map: RefCell::new([].into()),
			current_collection: RefCell::new(None),
			selected_index: Cell::new(None),

			selection,
			empty_callback: Box::new(empty_callback),
			notify: ModelNotify::default(),
		};
		Rc::new(result)
	}

	/// Updates the general state of the items table
	#[allow(clippy::too_many_arguments)]
	pub fn update(
		&self,
		info_db: Option<Option<Rc<InfoDb>>>,
		software_list_paths: Option<&[SmolStr]>,
		collection: Option<Rc<PrefsCollection>>,
		column_types: Option<Rc<[ColumnType]>>,
		search: Option<&str>,
		sorting: Option<Option<(ColumnType, SortOrder)>>,
		selection: Option<&[PrefsItemRef]>,
	) {
		// tracing
		let span = debug_span!("ItemsTableModel::update");
		let _guard = span.enter();
		debug!(
			?info_db,
			?software_list_paths,
			?collection,
			?column_types,
			?search,
			?sorting,
			?selection,
			"ItemsTableModel::update()"
		);

		// update the state that forces items refreshes
		let mut must_refresh_items = false;
		if let Some(info_db) = info_db {
			self.info_db.replace(info_db);
			must_refresh_items = true;
		}
		if let Some(software_list_paths) = software_list_paths
			&& software_list_paths != self.software_list_paths.borrow().as_slice()
		{
			self.software_list_paths.replace(software_list_paths.to_vec());
			must_refresh_items = true;
		}
		if let Some(collection) = collection
			&& Some(collection.as_ref()) != self.current_collection.borrow().as_deref()
		{
			self.current_collection.replace(Some(collection));
			must_refresh_items = true;
		}

		// update the state that forces the map to refresh
		let mut must_refresh_map = must_refresh_items;
		if let Some(search) = search
			&& search != self.search.borrow().as_str()
		{
			self.search.replace(search.to_string());
			must_refresh_map = true;
		}
		if let Some(sorting) = sorting
			&& sorting != self.sorting.get()
		{
			self.sorting.set(sorting);
			must_refresh_map = true;
		}

		// update the state that forces Slint model notifications
		let mut must_notify = must_refresh_map;
		if let Some(column_types) = column_types
			&& column_types.as_ref() != self.column_types.borrow().as_ref()
		{
			self.column_types.replace(column_types);
			must_notify = true;
		}

		// gauge whether we need to update the selection
		let selection = if let Some(selection) = selection {
			(must_refresh_map || selection != self.current_selection()).then_some(Cow::Borrowed(selection))
		} else {
			must_refresh_map.then(|| Cow::Owned(self.current_selection()))
		};

		// with all of that out of the way, do the actual refreshses
		if must_refresh_items {
			self.refresh_items();
		}
		if must_refresh_map {
			self.refresh_map();
		}
		if must_notify {
			self.notify.reset();
		}
		if let Some(selection) = selection {
			self.set_current_selection(selection.as_ref());
		}
	}

	fn refresh_items(&self) {
		debug!("ItemsTableModel::refresh_items()");
		let info_db = self.info_db.borrow();
		let collection = self.current_collection.borrow().clone();

		let (items, dispenser_is_empty) = info_db
			.as_ref()
			.map(|info_db: &Rc<InfoDb>| {
				let software_list_paths = self.software_list_paths.borrow();
				let mut dispenser = SoftwareListDispenser::new(info_db, &software_list_paths);

				let items = match collection.as_deref() {
					Some(PrefsCollection::Builtin(BuiltinCollection::All)) => Some(build_items_builtin_all(info_db)),
					Some(PrefsCollection::Builtin(BuiltinCollection::AllSoftware)) => {
						Some(build_items_builtin_all_software(&mut dispenser))
					}
					Some(PrefsCollection::MachineSoftware { machine_name }) => {
						build_items_machine_software(info_db, machine_name, &mut dispenser).ok()
					}
					Some(PrefsCollection::Folder { name: _, items }) => {
						Some(build_items_folder(info_db, items, &mut dispenser))
					}
					None => None,
				};
				(items.unwrap_or_default(), dispenser.is_empty())
			})
			.unwrap_or_else(|| (Rc::new([]), true));

		// if we're empty, try to gauge why and broadcast the result
		let empty_reason = items.is_empty().then(|| {
			if info_db.is_none() {
				EmptyReason::NoInfoDb
			} else if dispenser_is_empty || self.software_list_paths.borrow().is_empty() {
				EmptyReason::NoSoftwareLists
			} else if matches!(collection.as_deref(), Some(PrefsCollection::Folder { name: _, items }) if items.is_empty() )
			{
				EmptyReason::Folder
			} else {
				EmptyReason::Unknown
			}
		});
		(self.empty_callback)(empty_reason);

		// update the items
		self.items.replace(items);
	}

	pub fn context_commands(
		&self,
		row: usize,
		folder_info: &[(usize, Rc<PrefsCollection>)],
		has_mame_initialized: bool,
	) -> Option<ItemContextMenuInfo> {
		// access the InfoDB
		let info_db = self.info_db.borrow();
		let info_db = info_db.as_ref()?;

		// find the current folder (if any)
		let folder_name =
			if let Some(PrefsCollection::Folder { name, .. }) = &self.current_collection.borrow().as_deref() {
				Some(name.clone())
			} else {
				None
			};

		// access the selection
		let items = self.items.borrow();
		let index = *self.items_map.borrow().get(row).unwrap();
		let index = usize::try_from(index).unwrap();
		let item = items.get(index)?;

		// find the prefs item referenced by the Id, if appropriate
		let current_collection = self.current_collection.borrow().clone();
		let prefs_item = item.id.as_ref().and_then(|id| {
			current_collection
				.as_ref()
				.and_then(|collection| match collection.as_ref() {
					PrefsCollection::Folder { name: _, items } => {
						items.iter().find(|x| x.id.get_opt().as_ref() == Some(id))
					}
					_ => None,
				})
		});
		let video = prefs_item.and_then(|item| item.video.clone());

		// get the critical information - the description and where (if anyplace) "Browse" would go to
		let (run_title, run_descs, browse_target, can_configure) = match &item.details {
			ItemDetails::Machine {
				machine_config,
				images,
				ram_size,
				bios,
			} => {
				let machine = machine_config.machine();
				assert!(machine.runnable());
				let machine_name = machine.name().into();
				let ram_size = *ram_size;
				let bios = bios.clone();

				let slots = machine_config
					.changed_slots(None)
					.into_iter()
					.map(|(slot_name, slot_value)| {
						(format!("&{slot_name}").into(), slot_value.unwrap_or_default().into())
					})
					.collect::<Vec<_>>();
				let images = images
					.iter()
					.map(|(tag, image_desc)| (tag.as_str().into(), image_desc.clone()))
					.collect::<Vec<_>>();

				let start_args = MameStartArgs {
					machine_name,
					ram_size,
					bios,
					slots,
					images,
					video,
				};
				let action = Action::Start(start_args);
				let run_title = run_item_text(machine.description()).into();
				let run_descs = vec![MenuDesc {
					title: "".into(),
					action: Some(action),
				}];
				let browse_target =
					(!machine.machine_software_lists().is_empty()).then(|| PrefsCollection::MachineSoftware {
						machine_name: machine.name().to_string(),
					});

				(run_title, run_descs, browse_target, true)
			}
			ItemDetails::Software {
				software_list,
				software,
				machine_indexes,
				..
			} => {
				let run_descs = machine_indexes
					.iter()
					.filter_map(|&index| {
						// get the machine out of the InfoDB
						let machine = info_db.machines().get(index).unwrap();
						assert!(
							machine.runnable(),
							"software item {} from software list {}is associated with machine {} which is not runnable",
							software.name,
							software_list.name,
							machine.name()
						);

						// identify all parts of the software
						let parts_with_devices = software
							.parts
							.iter()
							.map(|part| {
								machine
									.devices()
									.iter()
									.find(|dev| dev.interfaces().any(|x| x == part.interface))
									.map(|dev| (dev.tag().into(), ImageDesc::Software(software.name.clone())))
									.ok_or(())
							})
							.collect::<std::result::Result<Vec<_>, ()>>();

						parts_with_devices.ok().map(|images| {
							let start_args = MameStartArgs {
								machine_name: machine.name().into(),
								ram_size: None,
								bios: None,
								slots: [].into(),
								images,
								video: video.clone(),
							};
							let action = Some(Action::Start(start_args));
							let title = machine.description().into();
							MenuDesc { action, title }
						})
					})
					.collect::<Vec<_>>();
				let run_title = run_item_text(&software.description).into();
				(run_title, run_descs, None, true)
			}
			ItemDetails::Unrecognized { error, .. } => {
				let run_title = error.to_string().into();
				let run_descs = Vec::new();
				(run_title, run_descs, None, false)
			}
		};

		// figure out the PrefsItem we would create if we were to add this to a folder
		let prefs_item = prefs_item.cloned().unwrap_or_else(|| {
			let details = match &item.details {
				ItemDetails::Machine { machine_config, .. } => PrefsItemDetails::Machine(PrefsMachineItem {
					machine_name: machine_config.machine().name().to_string(),
					slots: [].into(),
					images: [].into(),
					ram_size: None,
					bios: None,
				}),
				ItemDetails::Software {
					software_list,
					software,
					..
				} => {
					let software_list = software_list.name.as_str().into();
					let software = software.name.as_str().into();
					PrefsItemDetails::Software(PrefsSoftwareItem {
						software_list,
						software,
						preferred_machines: None,
					})
				}
				ItemDetails::Unrecognized { .. } => {
					unreachable!("Cannot create PrefsItemDetails for unrecognized item");
				}
			};
			PrefsItem {
				id: Default::default(),
				video: None,
				details,
			}
		});
		let prefs_items = Arc::<[_]>::from([prefs_item]);

		// now actually build the context menu
		let configure_action = can_configure
			.then_some(folder_name.as_ref())
			.flatten()
			.cloned()
			.map(|folder_name| Action::Configure { folder_name, index });
		let browse_action = browse_target.map(Action::Browse);

		// add to folder
		let add_to_existing_folder_descs = folder_info
			.iter()
			.map(|(index, col)| {
				let PrefsCollection::Folder {
					name,
					items: folder_items,
				} = &**col
				else {
					panic!("Expected PrefsCollection::Folder");
				};

				let folder_contains_all_items = prefs_items.iter().all(|x| folder_items.contains(x));
				let action =
					(!folder_contains_all_items).then(|| Action::AddToExistingFolder(*index, prefs_items.clone()));

				let title = name.into();
				MenuDesc { action, title }
			})
			.collect::<Vec<_>>();
		let new_folder_action = Action::AddToNewFolderDialog(prefs_items.clone());

		// remove from this folder
		let remove_from_folder_desc = folder_name.map(|folder_name| {
			let title = format!("Remove From \"{folder_name}\"").into();
			let ids = prefs_items.iter().filter_map(|item| item.id.get_opt()).collect::<_>();
			let action = Some(Action::RemoveFromFolder(folder_name, ids));
			MenuDesc { action, title }
		});

		// can't run if MAME is not initialized!
		let run_descs = if has_mame_initialized { run_descs } else { [].into() };

		// and return!
		let result = LocalItemContextMenuInfo {
			run_title,
			run_descs,
			configure_action,
			browse_action,
			add_to_existing_folder_descs,
			new_folder_action,
			remove_from_folder_desc,
		};
		Some(result.into())
	}

	fn refresh_map(&self) {
		debug!("ItemsTableModel::refresh_map()");

		// borrow all the things
		let info_db = self.info_db.borrow();
		let info_db = info_db.as_ref().map(|x| x.as_ref());
		let items = self.items.borrow();

		// build the new items map
		let new_items_map = build_items_map(
			info_db,
			&self.column_types.borrow(),
			&items,
			self.sorting.get(),
			&self.search.borrow(),
		);
		self.items_map.replace(new_items_map);
	}

	pub fn current_selection(&self) -> Vec<PrefsItemRef> {
		let result = self.current_selected_index().map(|index| {
			let items = self.items.borrow();
			let index = usize::try_from(index).unwrap();
			make_prefs_item_ref(&items[index])
		});

		result.into_iter().collect()
	}

	pub fn get_row_tooltip(&self, row: usize) -> Option<String> {
		let index = *self.items_map.borrow().get(row)?;
		let index = usize::try_from(index).unwrap();
		let items = self.items.borrow();
		let item = items.get(index)?;

		match &item.details {
			ItemDetails::Unrecognized { error, .. } => Some(error.to_string()),
			_ => None,
		}
	}

	pub fn row_text_by_column_type(&self, row: usize, column: ColumnType) -> Option<SharedString> {
		let info_db = self.info_db.borrow();
		let info_db = info_db.as_deref()?;
		let row = *self.items_map.borrow().get(row)?;
		let row = usize::try_from(row).unwrap();
		let items = self.items.borrow();
		let item = items.get(row)?;
		let text = column_text(info_db, item, column);
		Some(text.to_shared_string())
	}

	fn current_selected_index(&self) -> Option<u32> {
		self.selection
			.selected_index()
			.and_then(|x| self.items_map.borrow().get(x).cloned())
			.or_else(|| self.selected_index.get())
	}

	fn set_current_selection(&self, selection: &[PrefsItemRef]) {
		debug!(?selection, "ItemsTableModel::set_current_selection()");

		// we only support single selection now
		let selection = selection.iter().next();

		let selected_index = selection.and_then(|selection| {
			let items = self.items.borrow();
			let items_map = self.items_map.borrow();

			items_map
				.iter()
				.enumerate()
				.find(|(_, map_index)| {
					let map_index = usize::try_from(**map_index).unwrap();
					let item = &items[map_index];
					is_item_match(selection, item)
				})
				.map(|(index, _)| index)
		});

		self.selection.set_selected_index(selected_index);
	}
}

impl Model for ItemsTableModel {
	type Data = ModelRc<StandardListViewItem>;

	fn row_count(&self) -> usize {
		self.selection.model_accessed();
		self.items_map.borrow().len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let info_db = self.info_db.borrow().as_ref().unwrap().clone();
		let row = *self.items_map.borrow().get(row)?;
		let row = row.try_into().unwrap();
		let columns = self.column_types.borrow().clone();
		let items = self.items.borrow().clone();
		let row_model = RowModel::new(info_db, columns, items, row);
		Some(ModelRc::from(row_model))
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

trait SoftwareListDispenserTrait<'a> {
	fn get(&mut self, software_list_name: &str) -> Result<(crate::info::SoftwareList<'a>, Arc<SoftwareList>)>;
	fn get_all(&mut self) -> Vec<(crate::info::SoftwareList<'a>, Arc<SoftwareList>)>;
}

impl<'a> SoftwareListDispenserTrait<'a> for SoftwareListDispenser<'a> {
	fn get(&mut self, software_list_name: &str) -> Result<(crate::info::SoftwareList<'a>, Arc<SoftwareList>)> {
		self.get(software_list_name)
	}

	fn get_all(&mut self) -> Vec<(crate::info::SoftwareList<'a>, Arc<SoftwareList>)> {
		self.get_all()
	}
}

fn build_items_builtin_all(info_db: &Rc<InfoDb>) -> Rc<[Item]> {
	info_db
		.machines()
		.iter()
		.enumerate()
		.filter(|(_, machine)| machine.runnable())
		.map(|(machine_index, _)| {
			let machine_config = MachineConfig::from_machine_index(info_db.clone(), machine_index);
			let details = ItemDetails::Machine {
				machine_config,
				images: Default::default(),
				ram_size: None,
				bios: None,
			};
			details.into()
		})
		.collect::<Rc<[_]>>()
}

fn build_items_builtin_all_software<'a>(dispenser: &mut impl SoftwareListDispenserTrait<'a>) -> Rc<[Item]> {
	dispenser
		.get_all()
		.into_iter()
		.flat_map(|(info, list)| {
			list.software
				.iter()
				.map(|s| (list.clone(), s.clone(), info))
				.collect::<Vec<_>>()
		})
		.map(|(software_list, software, info)| {
			let machine_indexes = Iterator::chain(
				info.original_for_machines().iter(),
				info.compatible_for_machines().iter(),
			)
			.filter(|machine| machine.runnable())
			.map(|x| x.index())
			.collect();

			let details = ItemDetails::Software {
				software_list,
				software,
				machine_indexes,
			};
			details.into()
		})
		.collect::<Rc<[_]>>()
}

fn build_items_machine_software<'a>(
	info_db: &Rc<InfoDb>,
	machine_name: &str,
	dispenser: &mut impl SoftwareListDispenserTrait<'a>,
) -> Result<Rc<[Item]>> {
	let machine_index = info_db.machines().find_index(machine_name)?;
	let machine = info_db.machines().get(machine_index).unwrap();
	let items = machine
		.machine_software_lists()
		.iter()
		.filter_map(|x| dispenser.get(x.software_list().name()).ok())
		.flat_map(move |(_, list)| {
			(0..list.software.len()).map(move |index| (list.clone(), list.software[index].clone()))
		})
		.map(|(software_list, software)| {
			let machine_indexes = once(machine_index).collect();
			let details = ItemDetails::Software {
				software_list,
				software,
				machine_indexes,
			};
			details.into()
		})
		.collect::<Rc<[_]>>();
	Ok(items)
}

fn build_items_folder<'a>(
	info_db: &Rc<InfoDb>,
	folder_items: &[PrefsItem],
	dispenser: &mut impl SoftwareListDispenserTrait<'a>,
) -> Rc<[Item]> {
	folder_items
		.iter()
		.map(|item| {
			folder_item(info_db, dispenser, item).unwrap_or_else(|error| {
				let id = Some(item.id.get());
				let details = ItemDetails::Unrecognized {
					details: item.details.clone(),
					error: Rc::new(error),
				};
				Item { id, details }
			})
		})
		.collect::<Rc<[_]>>()
}

fn folder_item<'a>(
	info_db: &Rc<InfoDb>,
	dispenser: &mut impl SoftwareListDispenserTrait<'a>,
	prefs_item: &PrefsItem,
) -> Result<Item> {
	let details = match &prefs_item.details {
		PrefsItemDetails::Machine(item) => {
			let machine_config =
				MachineConfig::from_machine_name_and_slots(info_db.clone(), &item.machine_name, &item.slots)?;
			let images = item.images.clone();
			let ram_size = item.ram_size;
			let bios: Option<String> = item.bios.clone();
			ItemDetails::Machine {
				machine_config,
				images,
				ram_size,
				bios,
			}
		}
		PrefsItemDetails::Software(item) => {
			let (info, software_list) = dispenser.get(&item.software_list)?;
			let software = software_list
				.software
				.iter()
				.find(|x| x.name.as_str() == item.software)
				.ok_or_else(|| ThisError::UnknownSoftware(item.software.clone()))?
				.clone();

			let machine_indexes = if let Some(preferred_machines) = item.preferred_machines.as_deref() {
				preferred_machines
					.iter()
					.flat_map(|machine_name| info_db.machines().find(machine_name).ok())
					.map(|machine| machine.index())
					.collect()
			} else {
				Iterator::chain(
					info.original_for_machines().iter(),
					info.compatible_for_machines().iter(),
				)
				.map(|x| x.index())
				.collect()
			};

			ItemDetails::Software {
				software_list,
				software,
				machine_indexes,
			}
		}
	};
	let id = Some(prefs_item.id.get());
	Ok(Item { id, details })
}

/// Sometimes, the items view is empty - we can (try to) report why
#[derive(Clone, Copy, Debug, strum::Display)]
pub enum EmptyReason {
	#[strum(to_string = "BletchMAME needs a working MAME to function")]
	NoInfoDb,
	#[strum(to_string = "Unable to find any software lists")]
	NoSoftwareLists,
	#[strum(to_string = "This folder is empty")]
	Folder,
	#[strum(to_string = "Nothing to show for some reason!")]
	Unknown,
}

#[derive(Clone, Debug)]
struct Item {
	pub id: Option<SmolStr>,
	pub details: ItemDetails,
}

#[derive(Clone, Debug)]
enum ItemDetails {
	Machine {
		// commentary:  `MachineConfig` has its own `InfoDb`; maybe we need a lighter `MachineConfigPartial`?
		machine_config: MachineConfig,
		images: HashMap<String, ImageDesc>,
		ram_size: Option<u64>,
		bios: Option<String>,
	},
	Software {
		software_list: Arc<SoftwareList>,
		software: Arc<Software>,
		machine_indexes: SmallVec<[usize; 2]>,
	},
	Unrecognized {
		details: PrefsItemDetails,
		error: Rc<Error>,
	},
}

impl From<ItemDetails> for Item {
	fn from(details: ItemDetails) -> Self {
		Self { id: None, details }
	}
}

fn make_prefs_item_ref(item: &Item) -> PrefsItemRef {
	if let Some(id) = &item.id {
		PrefsItemRef::Id(id.clone())
	} else {
		match &item.details {
			ItemDetails::Machine { machine_config, .. } => {
				let machine_name = machine_config.machine().name().into();
				PrefsItemRef::Machine { machine_name }
			}
			ItemDetails::Software {
				software_list,
				software,
				..
			} => {
				let software_list = software_list.name.clone();
				let software = software.name.clone();
				PrefsItemRef::Software {
					software_list,
					software,
				}
			}
			ItemDetails::Unrecognized { .. } => {
				panic!("unrecognized item details without an id - cannot make PrefsItemRef");
			}
		}
	}
}

struct RowModel {
	info_db: Rc<InfoDb>,
	columns: Rc<[ColumnType]>,
	items: Rc<[Item]>,
	row: usize,
}

impl RowModel {
	pub fn new(info_db: Rc<InfoDb>, columns: Rc<[ColumnType]>, items: Rc<[Item]>, row: usize) -> Rc<Self> {
		Rc::new(Self {
			info_db,
			columns,
			items,
			row,
		})
	}
}

impl Model for RowModel {
	type Data = StandardListViewItem;

	fn row_count(&self) -> usize {
		self.columns.len()
	}

	fn row_data(&self, column: usize) -> Option<Self::Data> {
		let column = *self.columns.get(column)?;
		let item = self.items.get(self.row).unwrap();
		let text = column_text(&self.info_db, item, column).to_shared_string();
		Some(text.into())
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&()
	}
}

fn build_items_map(
	info_db: Option<&InfoDb>,
	column_types: &[ColumnType],
	items: &[Item],
	sorting: Option<(ColumnType, SortOrder)>,
	search: &str,
) -> Box<[u32]> {
	// tracing
	let span = info_span!("build_items_map");
	let _guard = span.enter();
	let start_instant = Instant::now();

	// do we have an InfoDb?
	let result = if let Some(info_db) = info_db {
		// start iterating
		let iter = items.iter().enumerate();

		// apply searching if appropriate
		let iter = if !search.is_empty() {
			let search_folded_case = search.to_folded_case();
			let iter = iter
				.filter_map(|(index, item)| {
					column_types
						.iter()
						.filter_map(|&column| {
							let text = column_text(info_db, item, column);
							let text_folded_case = text.to_folded_case();
							text_folded_case
								.contains(&search_folded_case)
								.then(|| levenshtein(&text, search))
						})
						.min()
						.map(|distance| (index, item, distance))
				})
				.sorted_by_key(|(_, _, distance)| *distance)
				.map(|(index, item, _)| (index, item));
			Either::Left(iter)
		} else {
			Either::Right(iter)
		};

		// now apply sorting
		let iter = if let Some((column_type, sort_order)) = sorting {
			let func = |item| UniCase::new(column_text(info_db, item, column_type));
			let iter = match sort_order {
				SortOrder::Ascending => Either::Left(iter.sorted_by_cached_key(|(_, item)| func(item))),
				SortOrder::Descending => Either::Right(iter.sorted_by_cached_key(|(_, item)| Reverse(func(item)))),
			};
			Either::Left(iter)
		} else {
			Either::Right(iter)
		};

		// and finish up
		iter.map(|(index, _)| u32::try_from(index).unwrap())
			.collect::<Box<[u32]>>()
	} else {
		// if we have no InfoDB, we have no rows
		[].into()
	};

	// and return
	debug!(duration=?start_instant.elapsed(), items_len=?items.len(), result_len=?result.len(), "build_items_map");
	result
}

fn column_text<'a>(_info_db: &'a InfoDb, item: &'a Item, column: ColumnType) -> Cow<'a, str> {
	match &item.details {
		ItemDetails::Machine { machine_config, .. } => {
			let machine = machine_config.machine();
			let text = match column {
				ColumnType::Name => machine.name(),
				ColumnType::SourceFile => machine.source_file(),
				ColumnType::Description => machine.description(),
				ColumnType::Year => machine.year(),
				ColumnType::Provider => machine.manufacturer(),
			};
			text.into()
		}
		ItemDetails::Software {
			software_list,
			software,
			..
		} => match column {
			ColumnType::Name => software.name.as_str().into(),
			ColumnType::SourceFile => format!("{}.xml", &software_list.name).into(),
			ColumnType::Description => software.description.as_str().into(),
			ColumnType::Year => software.year.as_str().into(),
			ColumnType::Provider => software.publisher.as_str().into(),
		},
		ItemDetails::Unrecognized { details, .. } => match details {
			PrefsItemDetails::Machine(item) => match column {
				ColumnType::Name => item.machine_name.as_str().into(),
				_ => "???".into(),
			},
			PrefsItemDetails::Software(item) => match column {
				ColumnType::Name => item.software.clone().into(),
				ColumnType::SourceFile => format!("{}.xml", item.software_list).into(),
				_ => "???".into(),
			},
		},
	}
}

fn is_item_match(prefs_item_ref: &PrefsItemRef, item: &Item) -> bool {
	make_prefs_item_ref(item) == *prefs_item_ref
}

fn run_item_text(text: &str) -> String {
	format!("Run {text}")
}

/// Rust friendly equivalent of ItemContextMenuInfo
struct LocalItemContextMenuInfo {
	run_title: SharedString,
	run_descs: Vec<MenuDesc>,
	configure_action: Option<Action>,
	browse_action: Option<Action>,
	add_to_existing_folder_descs: Vec<MenuDesc>,
	new_folder_action: Action,
	remove_from_folder_desc: Option<MenuDesc>,
}

struct MenuDesc {
	action: Option<Action>,
	title: SharedString,
}

impl From<LocalItemContextMenuInfo> for ItemContextMenuInfo {
	fn from(value: LocalItemContextMenuInfo) -> Self {
		let run_title = value.run_title;
		let run_descs = value
			.run_descs
			.into_iter()
			.map(MenuDesc::encode_for_slint)
			.collect::<Vec<_>>();
		let run_descs = VecModel::from(run_descs);
		let run_descs = ModelRc::new(run_descs);
		let configure_action = value
			.configure_action
			.as_ref()
			.map(Action::encode_for_slint)
			.unwrap_or_default();
		let browse_action = value
			.browse_action
			.as_ref()
			.map(Action::encode_for_slint)
			.unwrap_or_default();
		let add_to_existing_folder_descs = value
			.add_to_existing_folder_descs
			.into_iter()
			.map(MenuDesc::encode_for_slint)
			.collect::<Vec<_>>();
		let add_to_existing_folder_descs = VecModel::from(add_to_existing_folder_descs);
		let add_to_existing_folder_descs = ModelRc::new(add_to_existing_folder_descs);
		let new_folder_action = value.new_folder_action.encode_for_slint();
		let remove_from_folder_desc = value
			.remove_from_folder_desc
			.map(MenuDesc::encode_for_slint)
			.unwrap_or_default();
		Self {
			run_title,
			run_descs,
			configure_action,
			browse_action,
			add_to_existing_folder_descs,
			new_folder_action,
			remove_from_folder_desc,
		}
	}
}

impl MenuDesc {
	pub fn encode_for_slint(self) -> SimpleMenuEntry {
		let action = self.action.as_ref().map(Action::encode_for_slint).unwrap_or_default();
		let title = self.title;
		SimpleMenuEntry { action, title }
	}
}

#[cfg(test)]
mod test {
	use std::io::Cursor;
	use std::ops::ControlFlow;
	use std::rc::Rc;
	use std::sync::Arc;

	use anyhow::Result;
	use itertools::Itertools;
	use test_case::test_case;

	use crate::info::InfoDb;
	use crate::info::View;
	use crate::prefs::Preferences;
	use crate::prefs::PrefsCollection;
	use crate::selection::SelectionManager;
	use crate::software::SoftwareList;

	use super::ItemDetails;
	use super::ItemsTableModel;
	use super::SoftwareListDispenserTrait;

	struct MockSoftwareListDispenser<'a> {
		info_db: &'a InfoDb,
		software_lists: Vec<(crate::info::SoftwareList<'a>, Arc<SoftwareList>)>,
	}

	impl<'a> MockSoftwareListDispenser<'a> {
		pub fn new(info_db: &'a InfoDb) -> Self {
			let mut result = Self {
				info_db,
				software_lists: Vec::new(),
			};
			result.add(
				"coco_cart",
				include_str!("../software/test_data/softlist_coco_cart.xml"),
			);
			result
		}

		pub fn add(&mut self, name: &str, xml: &str) {
			let info_software_list = self.info_db.software_lists().find(name).unwrap();
			let software_list = SoftwareList::from_reader(xml.as_bytes()).unwrap();
			let software_list = Arc::new(software_list);
			self.software_lists.push((info_software_list, software_list));
		}
	}

	impl<'a> SoftwareListDispenserTrait<'a> for MockSoftwareListDispenser<'a> {
		fn get(&mut self, software_list_name: &str) -> Result<(crate::info::SoftwareList<'a>, Arc<SoftwareList>)> {
			let (info_software_list, software_list) = self
				.software_lists
				.iter()
				.find(|(info, _)| info.name() == software_list_name)
				.ok_or_else(|| anyhow::anyhow!("software list not found"))?;
			Ok((*info_software_list, software_list.clone()))
		}

		fn get_all(&mut self) -> Vec<(crate::info::SoftwareList<'a>, Arc<SoftwareList>)> {
			self.software_lists.clone()
		}
	}

	#[test_case(0, include_str!("../info/test_data/listxml_coco.xml"), include_str!("../prefs/test_data/prefs01.json"), "Favorites")]
	pub fn update_with_folder_count(_index: usize, info_xml: &str, prefs_xml: &str, folder_name: &str) {
		// set up the model
		let selection = SelectionManager::new_internal(|| 0, |_| {});
		let model = ItemsTableModel::new(selection, |_| {});

		// prepare an InfoDb
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		// get the prefs and find the folder
		let prefs_cursor = Cursor::new(prefs_xml);
		let prefs = Preferences::load_reader(prefs_cursor).unwrap();
		let collection = prefs
			.collections
			.into_iter()
			.filter(|collection| matches!(&**collection, PrefsCollection::Folder { name, .. } if name == folder_name))
			.exactly_one()
			.unwrap();

		// do the update
		model.update(
			Some(Some(info_db)),
			None,
			Some(collection.clone()),
			None,
			None,
			None,
			None,
		);

		// verify that the number of collections match
		let PrefsCollection::Folder { items, .. } = collection.as_ref() else {
			unreachable!()
		};
		let items_in_model = model.items.borrow().clone();
		assert_eq!(items.len(), items_in_model.len());
	}

	#[test_case(0, include_str!("../info/test_data/listxml_coco.xml"), "coco_cart", "baseball", &["coco", "coco2b", "coco2bh", "coco3", "coco3h", "coco3p", "cocoh"])]
	#[test_case(1, include_str!("../info/test_data/listxml_fake.xml"), "coco_cart", "baseball", &["fake"])]
	pub fn build_items_builtin_all_software(
		_index: usize,
		info_xml: &str,
		software_list_name: &str,
		software_name: &str,
		expected_machine_names: &[&str],
	) {
		// prepare an InfoDb and a mock software list dispenser
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);
		let mut dispenser = MockSoftwareListDispenser::new(&info_db);

		// get the item and validate
		let actual_items = super::build_items_builtin_all_software(&mut dispenser);
		let actual_item_machine_names = actual_items
			.iter()
			.filter_map(|item| {
				if let ItemDetails::Software {
					software_list,
					software,
					machine_indexes,
				} = &item.details
					&& software_list.name == software_list_name
					&& software.name == software_name
				{
					let machine_names = machine_indexes
						.iter()
						.map(|idx| info_db.machines().get(*idx).unwrap().name())
						.collect::<Vec<_>>();
					Some(machine_names)
				} else {
					None
				}
			})
			.exactly_one()
			.unwrap();
		assert_eq!(expected_machine_names, actual_item_machine_names.as_slice());
	}

	#[test_case(0, include_str!("../info/test_data/listxml_coco.xml"), "coco2b", 112)]
	pub fn build_items_machine_software(_index: usize, info_xml: &str, machine_name: &str, expected_item_count: usize) {
		// prepare an InfoDb and a mock software list dispenser
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);
		let mut dispenser = MockSoftwareListDispenser::new(&info_db);

		// get the items and validate
		let actual_items = super::build_items_machine_software(&info_db, machine_name, &mut dispenser).unwrap();
		assert_eq!(expected_item_count, actual_items.len());

		// ensure that each of these items are software items, and each only references the specified machine
		for item in actual_items.iter() {
			let ItemDetails::Software { machine_indexes, .. } = &item.details else {
				panic!("expected software item");
			};
			let actual_item_machine_names = machine_indexes
				.iter()
				.map(|idx| info_db.machines().get(*idx).unwrap().name())
				.collect::<Vec<_>>();
			assert_eq!(&[machine_name], actual_item_machine_names.as_slice());
		}
	}
}
