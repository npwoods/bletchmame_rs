use std::any::Any;
use std::borrow::Cow;
use std::cell::Cell;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::iter::once;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Error;
use anyhow::Result;
use itertools::Either;
use itertools::Itertools;
use levenshtein::levenshtein;
use muda::Menu;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::SharedString;
use slint::StandardListViewItem;
use tracing::Level;
use tracing::event;
use unicase::UniCase;

use crate::appcommand::AppCommand;
use crate::guiutils::menuing::MenuDesc;
use crate::info;
use crate::info::InfoDb;
use crate::info::View;
use crate::mconfig::MachineConfig;
use crate::prefs::BuiltinCollection;
use crate::prefs::ColumnType;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsColumn;
use crate::prefs::PrefsItem;
use crate::prefs::PrefsMachineItem;
use crate::prefs::SortOrder;
use crate::selection::SelectionManager;
use crate::software::Software;
use crate::software::SoftwareList;
use crate::software::SoftwareListDispenser;

const LOG: Level = Level::DEBUG;

pub struct ItemsTableModel {
	info_db: RefCell<Option<Rc<InfoDb>>>,
	software_list_paths: RefCell<Vec<String>>,
	columns: RefCell<Rc<[ColumnType]>>,
	sorting: Cell<Option<(ColumnType, SortOrder)>>,
	search: RefCell<String>,
	items: RefCell<Rc<[Item]>>,
	items_map: RefCell<Box<[u32]>>,

	current_collection: RefCell<Rc<PrefsCollection>>,
	selected_index: Cell<Option<u32>>,

	selection: SelectionManager,
	empty_callback: Box<dyn Fn(Option<EmptyReason>) + 'static>,
	ramsize_arg_string: Arc<str>,
	notify: ModelNotify,
}

impl ItemsTableModel {
	pub fn new(
		current_collection: Rc<PrefsCollection>,
		software_list_paths: Vec<String>,
		selection: SelectionManager,
		empty_callback: impl Fn(Option<EmptyReason>) + 'static,
	) -> Rc<Self> {
		let result = Self {
			info_db: RefCell::new(None),
			software_list_paths: RefCell::new(software_list_paths),
			columns: RefCell::new([].into()),
			sorting: Cell::new(None),
			search: RefCell::new("".into()),
			items: RefCell::new([].into()),
			items_map: RefCell::new([].into()),
			current_collection: RefCell::new(current_collection),
			selected_index: Cell::new(None),

			selection,
			empty_callback: Box::new(empty_callback),
			ramsize_arg_string: "-ramsize".into(),
			notify: ModelNotify::default(),
		};
		Rc::new(result)
	}

	pub fn info_db_changed(&self, info_db: Option<Rc<InfoDb>>) {
		self.info_db.replace(info_db);
		self.refresh(&[]);
	}

	pub fn set_current_collection(&self, collection: Rc<PrefsCollection>, search: String, selection: &[PrefsItem]) {
		self.current_collection.replace(collection);
		self.search.replace(search);
		self.refresh(selection);
	}

	pub fn set_software_list_paths(&self, software_list_paths: Vec<String>) {
		let selection = self.current_selection();
		self.software_list_paths.replace(software_list_paths);
		self.refresh(&selection);
	}

	fn refresh(&self, selection: &[PrefsItem]) {
		self.selected_index.set(None);
		let info_db = self.info_db.borrow();
		let collection = self.current_collection.borrow().clone();

		let (items, dispenser_is_empty) = info_db
			.as_ref()
			.map(|info_db: &Rc<InfoDb>| {
				let software_list_paths = self.software_list_paths.borrow();
				let mut dispenser = SoftwareListDispenser::new(info_db, &software_list_paths);

				let items = match collection.as_ref() {
					PrefsCollection::Builtin(BuiltinCollection::All) => {
						let machine_count = info_db.machines().len();
						(0..machine_count)
							.map(|machine_index| {
								let machine_config = MachineConfig::from_machine_index(info_db.clone(), machine_index);
								Item::Machine {
									machine_config,
									images: Default::default(),
									ram_size: None,
								}
							})
							.collect::<Rc<[_]>>()
					}
					PrefsCollection::Builtin(BuiltinCollection::AllSoftware) => dispenser
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
							}
						})
						.collect::<Rc<[_]>>(),

					PrefsCollection::MachineSoftware { machine_name } => info_db
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
						})
						.collect::<Rc<[_]>>(),

					PrefsCollection::Folder { name: _, items } => items
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
								let item = Item::Machine {
									machine_config,
									images,
									ram_size,
								};
								Some(item)
							}
							PrefsItem::Software {
								software_list,
								software,
							} => {
								let item = software_folder_item(&mut dispenser, software_list, software)
									.unwrap_or_else(|error| Item::UnrecognizedSoftware {
										software_list_name: software_list.clone(),
										software_name: software.clone(),
										error: Rc::new(error),
									});
								Some(item)
							}
						})
						.collect::<Rc<[_]>>(),
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
			} else if matches!(collection.as_ref(), PrefsCollection::Folder { name: _, items } if items.is_empty() ) {
				EmptyReason::Folder
			} else {
				EmptyReason::Unknown
			}
		});
		(self.empty_callback)(empty_reason);

		// update the items
		self.items.replace(items);
		self.update_items_map();

		// and reset the collection
		self.set_current_selection(selection);
	}

	pub fn context_commands(
		&self,
		index: usize,
		folder_info: &[(usize, Rc<PrefsCollection>)],
		has_mame_initialized: bool,
	) -> Option<Menu> {
		// access the InfoDB
		let info_db = self.info_db.borrow();
		let info_db = info_db.as_ref()?;

		// find the current folder (if any)
		let folder_name = if let PrefsCollection::Folder { name, .. } = &self.current_collection.borrow().as_ref() {
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
		let (run_menu_item, browse_target, configure_menu_item) = match item {
			Item::Machine {
				machine_config,
				images,
				ram_size,
			} => {
				let machine = machine_config.machine();
				let ramsize_arg = (
					self.ramsize_arg_string.clone(),
					ram_size.as_ref().map(u64::to_string).unwrap_or_default().into(),
				);
				let initial_loads = once(ramsize_arg)
					.chain(
						machine_config
							.changed_slots(None)
							.into_iter()
							.map(|(slot_name, slot_value)| {
								(format!("&{slot_name}").into(), slot_value.unwrap_or_default().into())
							}),
					)
					.chain(
						images
							.iter()
							.map(|(tag, filename)| (tag.as_str().into(), filename.as_str().into())),
					)
					.collect::<Vec<_>>();
				let command = has_mame_initialized.then(|| AppCommand::RunMame {
					machine_name: machine.name().to_string(),
					initial_loads,
				});
				let text = run_item_text(machine.description());
				let run_menu_item = MenuDesc::Item(text, command.map(|x| x.into()));
				let browse_target =
					(!machine.machine_software_lists().is_empty()).then(|| PrefsCollection::MachineSoftware {
						machine_name: machine.name().to_string(),
					});

				// items in folders can be configured
				let configure_menu_item = folder_name.clone().map(|folder_name| {
					let text = "Configure...".to_string();
					let command = AppCommand::ConfigureMachine { folder_name, index };
					MenuDesc::Item(text, Some(command.into()))
				});

				(run_menu_item, browse_target, configure_menu_item)
			}
			Item::Software {
				software,
				machine_indexes,
				..
			} => {
				let sub_items = machine_indexes
					.iter()
					.filter_map(|&index| {
						// get the machine out of the InfoDB
						let machine = info_db.machines().get(index).unwrap();

						// identify all parts of the software
						let parts_with_devices = software
							.parts
							.iter()
							.map(|part| {
								machine
									.devices()
									.iter()
									.find(|dev| dev.interfaces().any(|x| x == part.interface.as_ref()))
									.map(|dev| (Arc::<str>::from(dev.tag()), software.name.clone()))
									.ok_or(())
							})
							.collect::<std::result::Result<Vec<_>, ()>>();

						parts_with_devices.ok().map(|initial_loads| {
							// running is not yet supported!
							let command = AppCommand::RunMame {
								machine_name: machine.name().to_string(),
								initial_loads,
							};
							MenuDesc::Item(machine.description().to_string(), Some(command.into()))
						})
					})
					.collect::<Vec<_>>();
				let text = run_item_text(&software.description);
				let run_menu_item = MenuDesc::SubMenu(text, true, sub_items);
				(run_menu_item, None, None)
			}
			Item::UnrecognizedSoftware { error, .. } => {
				let message = format!("{}", error);
				let run_menu_item = MenuDesc::Item(message, None);
				(run_menu_item, None, None)
			}
		};

		// now actually build the context menu
		let mut menu_items = Vec::new();
		menu_items.push(run_menu_item);
		if let Some(configure_menu_item) = configure_menu_item {
			menu_items.push(configure_menu_item);
		}
		menu_items.push(MenuDesc::Separator);

		if let Some(browse_target) = browse_target {
			let id = AppCommand::Browse(browse_target).into();
			menu_items.push(MenuDesc::Item("Browse Software".to_string(), Some(id)));
		}

		// add to folder
		let mut folder_menu_items = folder_info
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
					(!folder_contains_all_items).then(|| AppCommand::AddToExistingFolder(*index, items.clone()).into());

				MenuDesc::Item(name.clone(), command)
			})
			.collect::<Vec<_>>();
		if !folder_menu_items.is_empty() {
			folder_menu_items.push(MenuDesc::Separator);
		}
		folder_menu_items.push(MenuDesc::Item(
			"New Folder...".into(),
			Some(AppCommand::AddToNewFolderDialog(items.clone()).into()),
		));
		menu_items.push(MenuDesc::SubMenu("Add To Folder".into(), true, folder_menu_items));

		// remove from this folder
		if let Some(folder_name) = folder_name {
			let text = format!("Remove From \"{}\"", folder_name);
			let command = AppCommand::RemoveFromFolder(folder_name, items.clone());
			menu_items.push(MenuDesc::Item(text, Some(command.into())));
		};

		// and return!
		Some(MenuDesc::make_popup_menu(menu_items))
	}

	pub fn set_columns_and_search(&self, columns: &[PrefsColumn], search: &str, sort_suppressed: bool) {
		// update columns
		self.columns.replace(columns.iter().map(|x| x.column_type).collect());

		// update search if it has changed
		let search_changed = search != *self.search.borrow();
		if search_changed {
			self.search.replace(search.to_string());
		}

		// determine the new sorting
		let sorting = (!sort_suppressed)
			.then(|| {
				columns
					.iter()
					.filter_map(|col| col.sort.map(|x| (col.column_type, x)))
					.next()
			})
			.flatten();
		let sorting_changed = sorting != self.sorting.get();
		if sorting_changed {
			self.sorting.set(sorting);
		}

		event!(
			LOG,
			"set_columns_and_search(): search={:?} sorting={:?} search_changed={} sorting_changed={:?}",
			search,
			sorting,
			search_changed,
			sorting_changed
		);

		// if anything changed, update our map
		if search_changed || sorting_changed {
			// get the selected index, because we're about to mess up all of the rows
			let selected_index = self.current_selected_index();

			self.update_items_map();

			// restore the selection
			let index = selected_index.and_then(|index| self.items_map.borrow().iter().position(|&x| index == x));
			self.selection.set_selected_index(index);
		}
	}

	fn update_items_map(&self) {
		// borrow all the things
		let info_db = self.info_db.borrow();
		let info_db = info_db.as_ref().map(|x| x.as_ref());
		let items = self.items.borrow();

		// build the new items map
		let new_items_map = build_items_map(
			info_db,
			&self.columns.borrow(),
			&items,
			self.sorting.get(),
			&self.search.borrow(),
		);
		self.items_map.replace(new_items_map);

		// and notify
		self.notify.reset();
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
		let columns = self.columns.borrow().clone();
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

fn software_folder_item(
	dispenser: &mut SoftwareListDispenser,
	software_list_name: &str,
	software_name: &str,
) -> Result<Item> {
	let (info, software_list) = dispenser.get(software_list_name)?;
	let software = software_list
		.software
		.iter()
		.find(|x| x.name.as_ref() == software_name)
		.ok_or_else(|| {
			let message = format!("Unknown software '{}'", software_name);
			Error::msg(message)
		})?
		.clone();

	Ok(software_item(info, software_list, software))
}

fn software_item(info: info::SoftwareList<'_>, software_list: Arc<SoftwareList>, software: Arc<Software>) -> Item {
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
	}
}

/// Sometimes, the items view is empty - we can (try to) report why
#[derive(Clone, Copy, Debug, strum_macros::Display)]
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
		images: HashMap<String, String>,
		ram_size: Option<u64>,
	},
	Software {
		software_list: Arc<SoftwareList>,
		software: Arc<Software>,
		machine_indexes: Vec<usize>,
	},
	UnrecognizedSoftware {
		software_list_name: String,
		software_name: String,
		error: Rc<Error>,
	},
}

fn make_prefs_item(_info_db: &InfoDb, item: &Item) -> PrefsItem {
	match item {
		Item::Machine {
			machine_config,
			images,
			ram_size,
		} => {
			let machine_name = machine_config.machine().name().to_string();
			let slots = machine_config.changed_slots(None);
			let images = images.clone();
			let ram_size = *ram_size;
			let item = PrefsMachineItem {
				machine_name,
				slots,
				images,
				ram_size,
			};
			PrefsItem::Machine(item)
		}
		Item::Software {
			software_list,
			software,
			..
		} => PrefsItem::Software {
			software_list: software_list.name.to_string(),
			software: software.name.to_string(),
		},
		Item::UnrecognizedSoftware {
			software_list_name,
			software_name,
			..
		} => PrefsItem::Software {
			software_list: software_list_name.clone(),
			software: software_name.clone(),
		},
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
	// if we have no InfoDB, we have no rows
	let Some(info_db) = info_db else {
		return [].into();
	};

	// start iterating
	let iter = items.iter().enumerate();

	// apply searching if appropriate
	let iter = if !search.is_empty() {
		let iter = iter
			.filter_map(|(index, item)| {
				let distance = column_types
					.iter()
					.filter_map(|&column| {
						let text = column_text(info_db, item, column);
						contains_and_distance(text.as_ref(), search)
					})
					.min();

				distance.map(|distance| (index, item, distance))
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
	iter.map(|(index, _)| u32::try_from(index).unwrap()).collect()
}

fn contains_and_distance(text: &str, target: &str) -> Option<usize> {
	text.to_lowercase()
		.contains(&target.to_lowercase())
		.then(|| levenshtein(text, target))
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
			ColumnType::Name => software.name.as_ref().into(),
			ColumnType::SourceFile => format!("{}.xml", &software_list.name).into(),
			ColumnType::Description => software.description.as_ref().into(),
			ColumnType::Year => software.year.as_ref().into(),
			ColumnType::Provider => software.publisher.as_ref().into(),
		},
		Item::UnrecognizedSoftware {
			software_list_name,
			software_name,
			..
		} => match column {
			ColumnType::Name => software_name.into(),
			ColumnType::SourceFile => format!("{}.xml", software_list_name).into(),
			_ => "".into(),
		},
	}
}

fn is_item_match(info_db: &InfoDb, prefs_item: &PrefsItem, item: &Item) -> bool {
	make_prefs_item(info_db, item) == *prefs_item
}

fn run_item_text(text: &str) -> String {
	format!("Run {}", text)
}
