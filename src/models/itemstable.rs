use std::any::Any;
use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::rc::Rc;

use derive_enum_all_values::AllValues;
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
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsItem;
use crate::software::Software;
use crate::software::SoftwareListDispenser;

pub struct ItemsTableModel {
	info_db: RefCell<Option<Rc<InfoDb>>>,
	software_list_paths: Vec<String>,
	items: RefCell<Rc<[Item]>>,
	items_map: RefCell<Box<[u32]>>,
	sorting_searching: RefCell<SortingSearching>,
	current_collection: RefCell<Rc<PrefsCollection>>,
	notify: ModelNotify,
}

impl ItemsTableModel {
	pub fn new(current_collection: Rc<PrefsCollection>, software_list_paths: Vec<String>) -> Rc<Self> {
		let sorting_searching = SortingSearching {
			column: Column::Name,
			order: SortOrder::Ascending,
			search: "".into(),
		};
		let result = Self {
			info_db: RefCell::new(None),
			software_list_paths,
			items: RefCell::new([].into()),
			items_map: RefCell::new([].into()),
			sorting_searching: RefCell::new(sorting_searching),
			current_collection: RefCell::new(current_collection),

			notify: ModelNotify::default(),
		};
		Rc::new(result)
	}

	pub fn info_db_changed(&self, info_db: Option<Rc<InfoDb>>) {
		self.info_db.replace(info_db);
		self.refresh();
	}

	pub fn browse(&self, collection: Rc<PrefsCollection>) {
		self.current_collection.replace(collection);
		self.refresh();
	}

	fn refresh(&self) {
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
					.flat_map(|x| x.software.clone())
					.map(|software| Item::Software { software })
					.collect::<Rc<[_]>>()
			}
			PrefsCollection::Folder { name: _, items } => items
				.iter()
				.filter_map(|item| match item {
					PrefsItem::Machine { machine_name } => info_db
						.machines()
						.find_index(machine_name)
						.map(|machine_index| Item::Machine { machine_index }),
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

	pub fn sort_ascending(&self, index: i32) {
		self.sort(index, SortOrder::Ascending);
	}
	pub fn sort_descending(&self, index: i32) {
		self.sort(index, SortOrder::Descending);
	}
	pub fn search_text_changed(&self, search: SharedString) {
		let new_sorting_searching = SortingSearching {
			search,
			..self.sorting_searching.borrow().clone()
		};
		self.change_sorting_searching(new_sorting_searching);
	}

	fn sort(&self, index: i32, order: SortOrder) {
		let Some(column) = usize::try_from(index)
			.ok()
			.and_then(|index| Column::all_values().get(index).cloned())
		else {
			return;
		};
		let new_sorting_searching = SortingSearching {
			column,
			order,
			..self.sorting_searching.borrow().clone()
		};
		self.change_sorting_searching(new_sorting_searching);
	}

	fn change_sorting_searching(&self, new_sorting_searching: SortingSearching) {
		let changed = {
			let mut sorting_searching = self.sorting_searching.borrow_mut();
			let changed = *sorting_searching != new_sorting_searching;
			if changed {
				*sorting_searching = new_sorting_searching;
			}
			changed
		};
		if changed {
			self.update_items_map();
		}
	}

	fn update_items_map(&self) {
		// borrow all the things
		let info_db = self.info_db.borrow();
		let info_db = info_db.as_ref().map(|x| x.as_ref());
		let items = self.items.borrow();
		let sorting_searching = self.sorting_searching.borrow();

		// build the new items map
		let new_items_map = build_items_map(info_db, &items, &sorting_searching);
		self.items_map.replace(new_items_map);

		// and notify
		self.notify.reset();
	}
}

impl Model for ItemsTableModel {
	type Data = ModelRc<StandardListViewItem>;

	fn row_count(&self) -> usize {
		self.items_map.borrow().len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let info_db = self.info_db.borrow().as_ref().unwrap().clone();
		let row = *self.items_map.borrow().get(row)?;
		let row = row.try_into().unwrap();
		let items = self.items.borrow().clone();
		let row_model = RowModel::new(info_db, items, row);
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
	Machine { machine_index: usize },
	Software { software: Rc<Software> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortOrder {
	Ascending,
	Descending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SortingSearching {
	column: Column,
	order: SortOrder,
	search: SharedString,
}

#[derive(AllValues, Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
	Name,
	SourceFile,
	Description,
	Year,
	Provider,
}

struct RowModel {
	info_db: Rc<InfoDb>,
	items: Rc<[Item]>,
	row: usize,
}

impl RowModel {
	pub fn new(info_db: Rc<InfoDb>, items: Rc<[Item]>, row: usize) -> Rc<Self> {
		Rc::new(Self { info_db, items, row })
	}
}

impl Model for RowModel {
	type Data = StandardListViewItem;

	fn row_count(&self) -> usize {
		Column::all_values().len()
	}

	fn row_data(&self, column: usize) -> Option<Self::Data> {
		let column = *Column::all_values().get(column)?;
		let item = self.items.get(self.row).unwrap();
		let text = column_text(&self.info_db, item, column);
		let text = String::from(text.as_ref());
		Some(SharedString::from(text).into())
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&()
	}
}

fn build_items_map(info_db: Option<&InfoDb>, items: &[Item], sorting_searching: &SortingSearching) -> Box<[u32]> {
	// if we have no InfoDB, we have no rows
	let Some(info_db) = info_db else {
		return [].into();
	};

	let search_string = sorting_searching.search.as_str().trim();
	if search_string.is_empty() {
		// sort by column text
		builds_item_map_sorted(info_db, items, sorting_searching.column, sorting_searching.order)
	} else {
		// sort by search string
		builds_item_map_search(info_db, items, search_string)
	}
}

fn builds_item_map_sorted(info_db: &InfoDb, items: &[Item], column: Column, order: SortOrder) -> Box<[u32]> {
	// prepare a sorting function as a lambda
	let func = |item| UniCase::new(column_text(info_db, item, column));

	// and do the dirty work
	let iter = items.iter().enumerate();
	let iter = match order {
		SortOrder::Ascending => Either::Left(iter.sorted_by_cached_key(|(_, item)| func(item))),
		SortOrder::Descending => Either::Right(iter.sorted_by_cached_key(|(_, item)| Reverse(func(item)))),
	};
	iter.collect::<Vec<_>>()
		.into_iter()
		.map(|(index, _)| index.try_into().unwrap())
		.collect()
}

fn builds_item_map_search(info_db: &InfoDb, items: &[Item], search_string: &str) -> Box<[u32]> {
	items
		.iter()
		.enumerate()
		.filter_map(|(index, item)| {
			let distance = Column::all_values()
				.iter()
				.filter_map(|&column| {
					let text = column_text(info_db, item, column);
					contains_and_distance(text.as_ref(), search_string)
				})
				.min();

			distance.map(|distance| (index, distance))
		})
		.sorted_by_key(|(_, distance)| *distance)
		.map(|(index, _)| index.try_into().unwrap())
		.collect()
}

fn contains_and_distance(text: &str, target: &str) -> Option<usize> {
	text.to_lowercase()
		.contains(&target.to_lowercase())
		.then(|| levenshtein(text, target))
}

fn column_text<'a>(info_db: &'a InfoDb, item: &'a Item, column: Column) -> Cow<'a, str> {
	match item {
		Item::Machine { machine_index } => {
			let machine = info_db.machines().get(*machine_index).unwrap();
			let text = match column {
				Column::Name => machine.name(),
				Column::SourceFile => machine.source_file(),
				Column::Description => machine.description(),
				Column::Year => machine.year(),
				Column::Provider => machine.manufacturer(),
			};
			text.into()
		}
		Item::Software { software } => {
			let text = match column {
				Column::Name => &software.name,
				Column::SourceFile => "",
				Column::Description => &software.description,
				Column::Year => &software.year,
				Column::Provider => &software.publisher,
			};
			text.into()
		}
	}
}
