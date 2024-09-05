use std::any::Any;
use std::borrow::Cow;
use std::cell::Cell;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::rc::Rc;
use std::sync::Arc;

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
use unicase::UniCase;

use crate::appcommand::AppCommand;
use crate::dialogs::file::PathType;
use crate::guiutils::menuing::MenuDesc;
use crate::info::InfoDb;
use crate::info::View;
use crate::prefs::BuiltinCollection;
use crate::prefs::ColumnType;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsColumn;
use crate::prefs::PrefsItem;
use crate::prefs::SortOrder;
use crate::selection::SelectionManager;
use crate::software::Software;
use crate::software::SoftwareList;
use crate::software::SoftwareListDispenser;

const LOG: bool = false;

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

		let (items, any_dispenser_failures) = {
			let software_list_paths = self.software_list_paths.borrow();
			let mut dispenser = SoftwareListDispenser::new(&software_list_paths);

			let items = info_db.as_ref().map(|info_db| match collection.as_ref() {
				PrefsCollection::Builtin(BuiltinCollection::All) => {
					let machine_count = info_db.machines().len();
					(0..machine_count)
						.map(|machine_index| Item::Machine { machine_index })
						.collect::<Rc<[_]>>()
				}
				PrefsCollection::Builtin(BuiltinCollection::AllSoftware) => dispenser
					.get_multiple(&info_db.software_lists().iter().map(|x| x.name()).collect::<Vec<_>>())
					.into_iter()
					.enumerate()
					.filter_map(|(index, software_list)| {
						software_list.map(|x| (info_db.software_lists().get(index).unwrap(), x))
					})
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
					.filter_map(|x| dispenser.get(&x.software_list().name()))
					.flat_map(|list| {
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
						PrefsItem::Machine { machine_name } => info_db
							.machines()
							.find_index(machine_name)
							.map(|machine_index| Item::Machine { machine_index }),
						PrefsItem::Software {
							software_list,
							software,
							machine_names,
						} => dispenser.get(software_list).and_then(|software_list| {
							software_list
								.software
								.iter()
								.find(|x| x.name.as_ref() == software)
								.cloned()
								.map(|software| {
									let machine_indexes = machine_names
										.iter()
										.filter_map(|x| info_db.machines().find(x))
										.map(|x| x.index())
										.collect::<Vec<_>>();
									Item::Software {
										software_list,
										software,
										machine_indexes,
									}
								})
						}),
					})
					.collect::<Rc<[_]>>(),
			});
			let items = items.unwrap_or_else(|| Rc::new([]));
			(items, dispenser.any_failures())
		};

		// if we're empty, try to gauge why and broadcast the result
		let empty_reason = items.is_empty().then(|| {
			if info_db.is_none() {
				EmptyReason::NoInfoDb
			} else if any_dispenser_failures || self.software_list_paths.borrow().is_empty() {
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

	pub fn context_commands(&self, index: usize, folder_info: &[(usize, Rc<PrefsCollection>)]) -> Option<Menu> {
		// access the InfoDB
		let info_db = self.info_db.borrow();
		let info_db = info_db.as_ref()?;

		// access the selection
		let items = self.items.borrow();
		let index = *self.items_map.borrow().get(index).unwrap();
		let index = usize::try_from(index).unwrap();
		let item = items.get(index)?;
		let items = vec![make_prefs_item(info_db, item)];

		// get the critical information - the description and where (if anyplace) "Browse" would go to
		let (description, machine_descriptions, browse_target) = match item {
			Item::Machine { machine_index } => {
				let machine = info_db.machines().get(*machine_index).unwrap();
				let description = Cow::from(machine.description());
				let browse_target =
					(!machine.machine_software_lists().is_empty()).then(|| PrefsCollection::MachineSoftware {
						machine_name: machine.name().to_string(),
					});
				(description, None, browse_target)
			}
			Item::Software {
				software,
				machine_indexes,
				..
			} => {
				let description = Cow::from(&*software.description);
				let machine_descriptions = machine_indexes
					.iter()
					.map(|&index| info_db.machines().get(index).unwrap().description().to_string())
					.collect::<Vec<_>>();
				(description, Some(machine_descriptions), None)
			}
		};

		// basics of
		let mut menu_items = Vec::new();
		let text = format!("Run \"{}\"", description);
		let item = if let Some(machine_descriptions) = machine_descriptions {
			let items = machine_descriptions
				.into_iter()
				.map(|text| MenuDesc::Item(text, None))
				.collect::<Vec<_>>();
			MenuDesc::SubMenu(text, true, items)
		} else {
			MenuDesc::Item(text, None)
		};
		menu_items.push(item);
		menu_items.push(MenuDesc::Separator);

		if let Some(browse_target) = browse_target {
			let id = AppCommand::Browse(browse_target).into();
			menu_items.push(MenuDesc::Item("Browse Software".to_string(), Some(id)));
		}

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
			Some(AppCommand::AddToNewFolderDialog(items).into()),
		));
		menu_items.push(MenuDesc::SubMenu("Add To Folder".into(), true, folder_menu_items));
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

		if LOG {
			println!(
				"set_columns_and_search(): search={:?} sorting={:?} search_changed={} sorting_changed={:?}",
				search, sorting, search_changed, sorting_changed
			);
		}

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

impl EmptyReason {
	pub fn action(&self) -> Option<(AppCommand, &'static str)> {
		match self {
			EmptyReason::NoInfoDb => Some((AppCommand::ChoosePath(PathType::MameExecutable), "Find MAME")),
			EmptyReason::NoSoftwareLists => Some((
				AppCommand::ChoosePath(PathType::SoftwareLists),
				"Find Software Lists...",
			)),
			_ => None,
		}
	}
}

#[derive(Clone)]
enum Item {
	Machine {
		machine_index: usize,
	},
	Software {
		software_list: Arc<SoftwareList>,
		software: Arc<Software>,
		machine_indexes: Vec<usize>,
	},
}

fn make_prefs_item(info_db: &InfoDb, item: &Item) -> PrefsItem {
	match item {
		Item::Machine { machine_index } => {
			let machine_name = info_db.machines().get(*machine_index).unwrap().name().to_string();
			PrefsItem::Machine { machine_name }
		}
		Item::Software {
			software_list,
			software,
			machine_indexes,
		} => {
			let software_list = software_list.name.to_string();
			let software = software.name.to_string();
			let machine_names = machine_indexes
				.iter()
				.map(|&index| info_db.machines().get(index).unwrap().name().to_string())
				.collect::<Vec<_>>();
			PrefsItem::Software {
				software_list,
				software,
				machine_names,
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

fn column_text<'a>(info_db: &'a InfoDb, item: &'a Item, column: ColumnType) -> Cow<'a, str> {
	match item {
		Item::Machine { machine_index } => {
			let machine = info_db.machines().get(*machine_index).unwrap();
			let text = match column {
				ColumnType::Name => machine.name(),
				ColumnType::SourceFile => machine.source_file(),
				ColumnType::Description => machine.description(),
				ColumnType::Year => machine.year(),
				ColumnType::Provider => machine.manufacturer(),
			};
			text.into()
		}
		Item::Software { software, .. } => {
			let text = match column {
				ColumnType::Name => &software.name,
				ColumnType::SourceFile => "",
				ColumnType::Description => &software.description,
				ColumnType::Year => &software.year,
				ColumnType::Provider => &software.publisher,
			};
			text.into()
		}
	}
}

fn is_item_match(info_db: &InfoDb, prefs_item: &PrefsItem, item: &Item) -> bool {
	make_prefs_item(info_db, item) == *prefs_item
}
