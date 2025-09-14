use std::any::Any;
use std::borrow::Cow;
use std::cell::Cell;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::HashMap;
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
use slint::VecModel;
use smol_str::SmolStr;
use tracing::debug;
use tracing::debug_span;
use tracing::info_span;
use unicase::UniCase;

use crate::appcommand::AppCommand;
use crate::imagedesc::ImageDesc;
use crate::info::InfoDb;
use crate::info::View;
use crate::mconfig::MachineConfig;
use crate::prefs::BuiltinCollection;
use crate::prefs::ColumnType;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsItem;
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
		selection: Option<&[PrefsItem]>,
	) {
		// tracing
		let span = debug_span!("ItemsTableModel::update");
		let _guard = span.enter();
		debug!(info_db=?info_db, software_list_paths=?software_list_paths, collection=?collection, column_types=?column_types, search=?search, sorting=?sorting, selection=?selection, "ItemsTableModel::update()");

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
					Some(PrefsCollection::Builtin(BuiltinCollection::All)) => info_db
						.machines()
						.iter()
						.enumerate()
						.filter(|(_, machine)| machine.runnable())
						.map(|(machine_index, _)| {
							let machine_config = MachineConfig::from_machine_index(info_db.clone(), machine_index);
							Item::Machine {
								machine_config,
								images: Default::default(),
								ram_size: None,
								bios: None,
							}
						})
						.collect::<Rc<[_]>>(),
					Some(PrefsCollection::Builtin(BuiltinCollection::AllSoftware)) => dispenser
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
							.map(|x| x.index())
							.collect::<Vec<_>>();

							Item::Software {
								software_list,
								software,
								machine_indexes,
								preferred_machines: None,
							}
						})
						.collect::<Rc<[_]>>(),

					Some(PrefsCollection::MachineSoftware { machine_name }) => info_db
						.machines()
						.find(machine_name)
						.into_iter()
						.flat_map(|x| x.machine_software_lists().iter().collect::<Vec<_>>())
						.filter_map(|x| dispenser.get(x.software_list().name()).ok())
						.flat_map(|(_, list)| {
							list.software
								.iter()
								.map(|s| (list.clone(), s.clone()))
								.collect::<Vec<_>>()
						})
						.map(|(software_list, software)| Item::Software {
							software_list,
							software,
							machine_indexes: Vec::default(),
							preferred_machines: None,
						})
						.collect::<Rc<[_]>>(),

					Some(PrefsCollection::Folder { name: _, items }) => items
						.iter()
						.filter_map(|item| match item {
							PrefsItem::Machine(item) => {
								let machine_config = MachineConfig::from_machine_name_and_slots(
									info_db.clone(),
									&item.machine_name,
									&item.slots,
								)
								.ok()?;
								let images = item.images.clone();
								let ram_size = item.ram_size;
								let bios = item.bios.clone();
								let item = Item::Machine {
									machine_config,
									images,
									ram_size,
									bios,
								};
								Some(item)
							}
							PrefsItem::Software(software_item) => {
								let item =
									software_folder_item(&mut dispenser, software_item).unwrap_or_else(|error| {
										Item::Unrecognized {
											item: item.clone(),
											error: Rc::new(error),
										}
									});
								Some(item)
							}
						})
						.collect::<Rc<[_]>>(),

					None => Rc::new([]),
				};
				(items, dispenser.is_empty())
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
		index: usize,
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
		let index = *self.items_map.borrow().get(index).unwrap();
		let index = usize::try_from(index).unwrap();
		let item = items.get(index)?;
		let items = vec![make_prefs_item(info_db, item)];

		// get the critical information - the description and where (if anyplace) "Browse" would go to
		let (run_title, run_descs, browse_target, can_configure) = match item {
			Item::Machine {
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
				};
				let command = has_mame_initialized.then_some(AppCommand::Start(start_args));
				let run_title = run_item_text(machine.description()).into();
				let run_descs = vec![MenuDesc {
					title: "".into(),
					command,
				}];
				let browse_target =
					(!machine.machine_software_lists().is_empty()).then(|| PrefsCollection::MachineSoftware {
						machine_name: machine.name().to_string(),
					});

				(run_title, run_descs, browse_target, true)
			}
			Item::Software {
				software,
				machine_indexes,
				..
			} => {
				let run_descs = machine_indexes
					.iter()
					.filter_map(|&index| {
						// get the machine out of the InfoDB
						let machine = info_db.machines().get(index).unwrap();
						assert!(machine.runnable());

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
							};
							let command = Some(AppCommand::Start(start_args));
							let title = machine.description().into();
							MenuDesc { command, title }
						})
					})
					.collect::<Vec<_>>();
				let run_title = run_item_text(&software.description).into();
				(run_title, run_descs, None, true)
			}
			Item::Unrecognized { error, .. } => {
				let run_title = error.to_string().into();
				let run_descs = Vec::new();
				(run_title, run_descs, None, false)
			}
		};

		// now actually build the context menu
		let configure_command = can_configure
			.then_some(folder_name.as_ref())
			.flatten()
			.cloned()
			.map(|folder_name| AppCommand::Configure { folder_name, index });
		let browse_command = browse_target.map(AppCommand::Browse);

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

				let folder_contains_all_items = items.iter().all(|x| folder_items.contains(x));
				let command =
					(!folder_contains_all_items).then(|| AppCommand::AddToExistingFolder(*index, items.clone()));

				let title = name.into();
				MenuDesc { command, title }
			})
			.collect::<Vec<_>>();
		let new_folder_command = AppCommand::AddToNewFolderDialog(items.clone());

		// remove from this folder
		let remove_from_folder_desc = folder_name.map(|folder_name| {
			let title = format!("Remove From \"{folder_name}\"").into();
			let command = Some(AppCommand::RemoveFromFolder(folder_name, items.clone()));
			MenuDesc { command, title }
		});

		// and return!
		let result = LocalItemContextMenuInfo {
			run_title,
			run_descs,
			configure_command,
			browse_command,
			add_to_existing_folder_descs,
			new_folder_command,
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

	pub fn current_selection(&self) -> Vec<PrefsItem> {
		// if we have no InfoDB, we have no SELECTION
		let info_db = self.info_db.borrow();
		let Some(info_db) = &*info_db else {
			return [].into();
		};

		let result = self.current_selected_index().map(|index| {
			let items = self.items.borrow();
			let index = usize::try_from(index).unwrap();
			make_prefs_item(info_db, &items[index])
		});

		result.into_iter().collect()
	}

	fn current_selected_index(&self) -> Option<u32> {
		self.selection
			.selected_index()
			.and_then(|x| self.items_map.borrow().get(x).cloned())
			.or_else(|| self.selected_index.get())
	}

	fn set_current_selection(&self, selection: &[PrefsItem]) {
		debug!(selection=?selection, "ItemsTableModel::set_current_selection()");
		let info_db = self.info_db.borrow();
		let Some(info_db) = &*info_db else {
			return;
		};

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
					is_item_match(info_db, selection, item)
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

fn software_folder_item(dispenser: &mut SoftwareListDispenser, item: &PrefsSoftwareItem) -> Result<Item> {
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
			.flat_map(|machine_name| dispenser.info_db.machines().find(machine_name).ok())
			.map(|machine| machine.index())
			.collect::<Vec<_>>()
	} else {
		Iterator::chain(
			info.original_for_machines().iter(),
			info.compatible_for_machines().iter(),
		)
		.map(|x| x.index())
		.collect::<Vec<_>>()
	};

	let preferred_machines = item.preferred_machines.as_ref().map(|x| x.iter().join("\0").into());

	let result = Item::Software {
		software_list,
		software,
		machine_indexes,
		preferred_machines,
	};
	Ok(result)
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

#[derive(Clone)]
enum Item {
	Machine {
		// Commentary:  `MachineConfig` has its own `InfoDb`; maybe we need a lighter `MachineConfigPartial`?
		machine_config: MachineConfig,
		images: HashMap<String, ImageDesc>,
		ram_size: Option<u64>,
		bios: Option<String>,
	},
	Software {
		software_list: Arc<SoftwareList>,
		software: Arc<Software>,
		machine_indexes: Vec<usize>,
		preferred_machines: Option<Box<str>>, // NUL delimited
	},
	Unrecognized {
		item: PrefsItem,
		error: Rc<Error>,
	},
}

fn make_prefs_item(_info_db: &InfoDb, item: &Item) -> PrefsItem {
	match item {
		Item::Machine {
			machine_config,
			images,
			ram_size,
			bios,
		} => {
			let machine_name = machine_config.machine().name().to_string();
			let slots = machine_config.changed_slots(None);
			let images = images.clone();
			let ram_size = *ram_size;
			let bios = bios.clone();
			let item = PrefsMachineItem {
				machine_name,
				slots,
				images,
				ram_size,
				bios,
			};
			PrefsItem::Machine(item)
		}
		Item::Software {
			software_list,
			software,
			preferred_machines,
			..
		} => {
			let preferred_machines = preferred_machines
				.as_ref()
				.map(|x| x.split('\0').map(str::to_string).collect::<Vec<_>>());
			let item = PrefsSoftwareItem {
				software_list: software_list.name.to_string(),
				software: software.name.to_string(),
				preferred_machines,
			};
			PrefsItem::Software(item)
		}
		Item::Unrecognized { item, .. } => item.clone(),
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
		let text = column_text(&self.info_db, item, column);
		let text = String::from(text.as_ref());
		Some(SharedString::from(text).into())
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
	match item {
		Item::Machine { machine_config, .. } => {
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
		Item::Software {
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
		Item::Unrecognized { item, .. } => {
			let PrefsItem::Software(item) = item else { todo!() };
			match column {
				ColumnType::Name => item.software.clone().into(),
				ColumnType::SourceFile => format!("{}.xml", item.software_list).into(),
				_ => "".into(),
			}
		}
	}
}

fn is_item_match(info_db: &InfoDb, prefs_item: &PrefsItem, item: &Item) -> bool {
	make_prefs_item(info_db, item) == *prefs_item
}

fn run_item_text(text: &str) -> String {
	format!("Run {text}")
}

/// Rust friendly equivalent of ItemContextMenuInfo
struct LocalItemContextMenuInfo {
	run_title: SharedString,
	run_descs: Vec<MenuDesc>,
	configure_command: Option<AppCommand>,
	browse_command: Option<AppCommand>,
	add_to_existing_folder_descs: Vec<MenuDesc>,
	new_folder_command: AppCommand,
	remove_from_folder_desc: Option<MenuDesc>,
}

struct MenuDesc {
	command: Option<AppCommand>,
	title: SharedString,
}

impl From<LocalItemContextMenuInfo> for ItemContextMenuInfo {
	fn from(value: LocalItemContextMenuInfo) -> Self {
		let run_title = value.run_title;
		let run_descs = value
			.run_descs
			.into_iter()
			.map(|desc| {
				let command = desc
					.command
					.as_ref()
					.map(AppCommand::encode_for_slint)
					.unwrap_or_default();
				let title = desc.title;
				SimpleMenuEntry { command, title }
			})
			.collect::<Vec<_>>();
		let run_descs = VecModel::from(run_descs);
		let run_descs = ModelRc::new(run_descs);
		let configure_command = value
			.configure_command
			.as_ref()
			.map(AppCommand::encode_for_slint)
			.unwrap_or_default();
		let browse_command = value
			.browse_command
			.as_ref()
			.map(AppCommand::encode_for_slint)
			.unwrap_or_default();
		let add_to_existing_folder_descs = value
			.add_to_existing_folder_descs
			.into_iter()
			.map(MenuDesc::encode_for_slint)
			.collect::<Vec<_>>();
		let add_to_existing_folder_descs = VecModel::from(add_to_existing_folder_descs);
		let add_to_existing_folder_descs = ModelRc::new(add_to_existing_folder_descs);
		let new_folder_command = value.new_folder_command.encode_for_slint();
		let remove_from_folder_desc = value
			.remove_from_folder_desc
			.map(MenuDesc::encode_for_slint)
			.unwrap_or_default();
		Self {
			run_title,
			run_descs,
			configure_command,
			browse_command,
			add_to_existing_folder_descs,
			new_folder_command,
			remove_from_folder_desc,
		}
	}
}

impl MenuDesc {
	pub fn encode_for_slint(self) -> (SharedString, SharedString) {
		(
			self.command
				.as_ref()
				.map(AppCommand::encode_for_slint)
				.unwrap_or_default(),
			self.title,
		)
	}
}
