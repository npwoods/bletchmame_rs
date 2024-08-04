use std::any::Any;
use std::borrow::Cow;
use std::cell::Cell;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::rc::Rc;

use itertools::Either;
use itertools::Itertools;
use levenshtein::levenshtein;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::SharedString;
use slint::StandardListViewItem;
use unicase::UniCase;

use crate::appcommand::AppCommand;
use crate::info::InfoDb;
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
	software_list_paths: Vec<String>,
	columns: RefCell<Rc<[ColumnType]>>,
	sorting: Cell<Option<(ColumnType, SortOrder)>>,
	search: RefCell<String>,
	items: RefCell<Rc<[Item]>>,
	items_map: RefCell<Box<[u32]>>,

	current_collection: RefCell<Rc<PrefsCollection>>,
	selected_index: Cell<Option<u32>>,

	selection: SelectionManager,
	notify: ModelNotify,
}

impl ItemsTableModel {
	pub fn new(
		current_collection: Rc<PrefsCollection>,
		software_list_paths: Vec<String>,
		selection: SelectionManager,
	) -> Rc<Self> {
		let result = Self {
			info_db: RefCell::new(None),
			software_list_paths,
			columns: RefCell::new([].into()),
			sorting: Cell::new(None),
			search: RefCell::new("".into()),
			items: RefCell::new([].into()),
			items_map: RefCell::new([].into()),
			current_collection: RefCell::new(current_collection),
			selected_index: Cell::new(None),

			selection,
			notify: ModelNotify::default(),
		};
		Rc::new(result)
	}

	pub fn info_db_changed(&self, info_db: Option<Rc<InfoDb>>) {
		self.info_db.replace(info_db);
		self.refresh();
	}

	pub fn set_current_collection(&self, collection: Rc<PrefsCollection>, search: String, selection: &[PrefsItem]) {
		self.current_collection.replace(collection);
		self.search.replace(search);
		self.refresh();

		self.set_current_selection(selection);
	}

	fn refresh(&self) {
		self.selected_index.set(None);
		let info_db = self.info_db.borrow();
		let collection = self.current_collection.borrow().clone();

		let items = info_db.as_ref().map(|info_db| match collection.as_ref() {
			PrefsCollection::Builtin(BuiltinCollection::All) => {
				let machine_count = info_db.machines().len();
				(0..machine_count)
					.map(|machine_index| Item::Machine { machine_index })
					.collect::<Rc<[_]>>()
			}
			PrefsCollection::MachineSoftware { machine_name } => {
				let mut dispenser = SoftwareListDispenser::new(&self.software_list_paths);
				info_db
					.machines()
					.find(machine_name)
					.into_iter()
					.flat_map(|x| x.software_lists().iter())
					.filter_map(|x| dispenser.get(&x.name()))
					.flat_map(|list| {
						list.software
							.iter()
							.map(|s| (list.clone(), s.clone()))
							.collect::<Vec<_>>()
					})
					.map(|(software_list, software)| Item::Software {
						software_list,
						software,
					})
					.collect::<Rc<[_]>>()
			}
			PrefsCollection::Folder { name: _, items } => items
				.iter()
				.filter_map(|item| match item {
					PrefsItem::Machine { machine_name } => info_db
						.machines()
						.find_index(machine_name)
						.map(|machine_index| Item::Machine { machine_index }),
					PrefsItem::Software { .. } => todo!(),
				})
				.collect::<Rc<[_]>>(),
		});
		let items = items.unwrap_or_else(|| Rc::new([]));
		self.items.replace(items);
		self.update_items_map();
	}

	pub fn context_commands(&self, index: usize) -> impl Iterator<Item = AppCommand> {
		let items = self.items.borrow();
		let info_db = self.info_db.borrow();
		let index = *self.items_map.borrow().get(index).unwrap();
		let index = usize::try_from(index).unwrap();
		let item = items.get(index);

		let commands = match item.as_ref() {
			Some(Item::Machine { machine_index }) => {
				info_db.as_ref().unwrap().machines().get(*machine_index).map(|machine| {
					let collection = PrefsCollection::MachineSoftware {
						machine_name: machine.name().into(),
					};
					vec![AppCommand::Browse(collection)]
				})
			}
			Some(Item::Software { .. }) => todo!(),
			None => None,
		};

		commands.unwrap_or_default().into_iter()
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
			self.update_items_map();
		}
	}

	fn update_items_map(&self) {
		// get the selected index, because we're about to mess up all of the rows
		let selected_index = self.current_selected_index();

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

		// restore the selection
		let index = selected_index.and_then(|index| self.items_map.borrow().iter().position(|&x| index == x));
		self.selection.set_selected_index(index);
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

			match &items[index] {
				Item::Machine { machine_index } => {
					let machine_name = info_db.machines().get(*machine_index).unwrap().name().to_string();
					PrefsItem::Machine { machine_name }
				}
				Item::Software {
					software_list,
					software,
				} => {
					let software_list = software_list.name.to_string();
					let software = software.name.to_string();
					PrefsItem::Software {
						software_list,
						software,
					}
				}
			}
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

enum Item {
	Machine {
		machine_index: usize,
	},
	Software {
		software_list: Rc<SoftwareList>,
		software: Rc<Software>,
	},
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
	match (prefs_item, item) {
		(PrefsItem::Machine { machine_name }, Item::Machine { machine_index }) => {
			machine_name == &info_db.machines().get(*machine_index).unwrap().name().to_string()
		}
		(
			PrefsItem::Software {
				software_list: a_software_list,
				software: a_software,
			},
			Item::Software {
				software_list: b_software_list,
				software: b_software,
			},
		) => (a_software_list == &*b_software_list.name) && (a_software == &*b_software.name),
		_ => false,
	}
}
