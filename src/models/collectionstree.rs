use std::default::Default;
use std::rc::Rc;

use itertools::Itertools;
use slint::SharedString;

use crate::info::InfoDb;
use crate::info::Machine;
use crate::info::SmallStrRef;
use crate::models::itemstable::Item;
use crate::models::tree::TreeModel;
use crate::prefs::BuiltinCollectionItem;
use crate::prefs::FolderCollectionItem;
use crate::prefs::InnerCollectionItem;
use crate::prefs::PrefsCollectionItem;
use crate::prefs::PrefsSelection;
use crate::ui::TreeNode;

pub type CollectionTreeModel = TreeModel<Rc<dyn CollectionNode>>;
type Entry = crate::models::tree::Entry<Rc<dyn CollectionNode>>;

impl CollectionTreeModel {
	pub fn update(&self, info_db: Option<Rc<InfoDb>>, prefs: &[PrefsCollectionItem]) {
		update_collections_tree_model(self, info_db, prefs)
	}

	pub fn get_selected_items(&self) -> Option<Rc<[Item]>> {
		self.selected_data().map(|x| x.get_items())
	}

	pub fn get_prefs(&self) -> Vec<PrefsCollectionItem> {
		get_prefs_from_entries(&self.entries.borrow())
	}
}

fn update_collections_tree_model(
	model: &CollectionTreeModel,
	info_db: Option<Rc<InfoDb>>,
	prefs: &[PrefsCollectionItem],
) {
	// start building entries to feed into the tree view
	let mut entries = Vec::new();

	// things are actually interesting when we have an InfoDb
	if let Some(info_db) = info_db.as_ref() {
		build_entries_for_prefs(&mut entries, 0, info_db, prefs);
	}

	// take note of whether we have any items
	let has_items = !entries.is_empty();

	// load things in
	model.load(entries);

	// if we're not selecting anything but we have items, select the first item
	if !model.has_selection() && has_items {
		model.set_selected_index(Some(0));
	}
}

/// recursive function to build all entries
fn build_entries_for_prefs(
	entries: &mut Vec<(TreeNode, Rc<dyn CollectionNode>)>,
	indentation: i32,
	info_db: &Rc<InfoDb>,
	prefs: &[PrefsCollectionItem],
) {
	for pref in prefs {
		// we need to duplicate the item separate from any children
		let (pref_dup, pref_children) = match &pref.inner {
			InnerCollectionItem::Folder(x) => {
				let name = x.name.clone();
				(
					InnerCollectionItem::Folder(FolderCollectionItem {
						name,
						children: Default::default(),
					}),
					x.children.as_slice(),
				)
			}
			_ => (pref.inner.clone(), Default::default()),
		};

		// create the node
		let node = PrefsCollectionNode::new(pref_dup, info_db.clone());
		let node = Rc::new(node) as Rc<dyn CollectionNode>;

		// and create the entry
		let text = node.text();
		let is_selected = matches!(pref.selected, PrefsSelection::Bool(true));
		let node_children = node.children();
		let display = TreeNode {
			has_children: !pref_children.is_empty() || !node_children.is_empty(),
			indentation,
			text,
			is_selected,
			is_open: false,
		};
		entries.push((display, node));

		// add any child preferences
		build_entries_for_prefs(entries, indentation + 1, info_db, pref_children);

		// node children are treated differently
		for child in node_children {
			let text = child.text();
			let is_selected = matches!(&pref.selected, PrefsSelection::String(x) if **x == *text);
			let display = TreeNode {
				has_children: false,
				indentation: indentation + 1,
				text,
				is_selected,
				is_open: false,
			};
			entries.push((display, child));
		}
	}
}

fn get_prefs_from_entries(entries: &[Entry]) -> Vec<PrefsCollectionItem> {
	entries
		.iter()
		.enumerate()
		.map(|(index, entry)| (index, entry, (index + 1..(index + 1))))
		.coalesce(|a, b| {
			let (a_index, a_entry, a_children_range) = a.clone();
			let (_b_index, b_entry, b_children_range) = b.clone();
			if a_entry.display.indentation < b_entry.display.indentation {
				Ok((a_index, a_entry, (a_children_range.start)..(b_children_range.end)))
			} else {
				Err((a, b))
			}
		})
		.map(|(_, entry, children_range)| {
			let children = &entries[children_range];
			entry.data.get_single_pref(entry, children)
		})
		.collect()
}

pub trait CollectionNode {
	fn text(&self) -> SharedString;
	fn children(&self) -> Vec<Rc<dyn CollectionNode>> {
		Default::default()
	}
	fn get_items(&self) -> Rc<[Item]> {
		[].into()
	}
	fn get_single_pref(&self, _entry: &Entry, _children: &[Entry]) -> PrefsCollectionItem {
		panic!("Should not get here")
	}
}

struct PrefsCollectionNode {
	item: InnerCollectionItem,
	info_db: Rc<InfoDb>,
}

impl PrefsCollectionNode {
	pub fn new(item: InnerCollectionItem, info_db: Rc<InfoDb>) -> Self {
		Self { item, info_db }
	}
}

impl CollectionNode for PrefsCollectionNode {
	fn text(&self) -> SharedString {
		match &self.item {
			InnerCollectionItem::Builtin(x) => match x {
				BuiltinCollectionItem::All => "All Systems".into(),
				BuiltinCollectionItem::Source => "Source".into(),
				BuiltinCollectionItem::Year => "Year".into(),
				BuiltinCollectionItem::Manufacturer => "Manufacturer".into(),
			},
			InnerCollectionItem::Machines(x) => x.name.as_ref().map_or_else(
				|| {
					x.machines
						.first()
						.and_then(|machine_name| {
							self.info_db
								.machines()
								.find(machine_name)
								.map(|m| SharedString::from(m.description()))
						})
						.unwrap_or_default()
				},
				SharedString::from,
			),
			InnerCollectionItem::Software(_) => "Software".into(),
			InnerCollectionItem::Folder(x) => x.name.as_str().into(),
		}
	}

	fn children(&self) -> Vec<Rc<dyn CollectionNode>> {
		match &self.item {
			InnerCollectionItem::Builtin(BuiltinCollectionItem::Source) => {
				MachineSubsetCollectionNode::new_nodes_vec(&self.info_db, |machine| machine.source_file())
			}
			InnerCollectionItem::Builtin(BuiltinCollectionItem::Year) => {
				MachineSubsetCollectionNode::new_nodes_vec(&self.info_db, |machine| machine.year())
			}
			InnerCollectionItem::Builtin(BuiltinCollectionItem::Manufacturer) => {
				MachineSubsetCollectionNode::new_nodes_vec(&self.info_db, |machine| machine.manufacturer())
			}
			_ => Default::default(),
		}
	}

	fn get_items(&self) -> Rc<[Item]> {
		match &self.item {
			InnerCollectionItem::Builtin(x) => match x {
				BuiltinCollectionItem::All
				| BuiltinCollectionItem::Source
				| BuiltinCollectionItem::Year
				| BuiltinCollectionItem::Manufacturer => items_from_machines(&self.info_db, |_| true),
			},
			_ => [].into(),
		}
	}

	fn get_single_pref(&self, entry: &Entry, children: &[Entry]) -> PrefsCollectionItem {
		let (inner, selected_child) = if let InnerCollectionItem::Folder(x) = &self.item {
			let inner = InnerCollectionItem::Folder(FolderCollectionItem {
				name: x.name.clone(),
				children: get_prefs_from_entries(children),
			});
			(inner, None)
		} else {
			let inner = self.item.clone();
			let selected_child = children
				.iter()
				.find(|x| x.display.is_selected)
				.map(|x| x.display.text.to_string());
			(inner, selected_child)
		};

		let selected = if let Some(selected_child) = selected_child {
			PrefsSelection::String(selected_child)
		} else {
			PrefsSelection::Bool(entry.display.is_selected)
		};

		PrefsCollectionItem { selected, inner }
	}
}

fn items_from_machines(info_db: &InfoDb, predicate: impl Fn(Machine) -> bool) -> Rc<[Item]> {
	info_db
		.machines()
		.iter()
		.enumerate()
		.filter(|(_, machine)| machine.runnable() && predicate(*machine))
		.map(|(index, _)| Item::Machine {
			machine_index: index.try_into().unwrap(),
		})
		.collect()
}

struct MachineSubsetCollectionNode {
	info_db: Rc<InfoDb>,
	selector: for<'a> fn(Machine<'a>) -> SmallStrRef<'a>,
	text: SharedString,
}

impl MachineSubsetCollectionNode {
	pub fn new(info_db: Rc<InfoDb>, selector: for<'a> fn(Machine<'a>) -> SmallStrRef<'a>, text: SharedString) -> Self {
		Self {
			info_db,
			selector,
			text,
		}
	}

	pub fn new_nodes_vec(
		info_db: &Rc<InfoDb>,
		selector: for<'a> fn(Machine<'a>) -> SmallStrRef<'a>,
	) -> Vec<Rc<dyn CollectionNode>> {
		info_db
			.machines()
			.iter()
			.filter(|machine| machine.runnable())
			.map(selector)
			.unique()
			.sorted()
			.map(|text| {
				let node = MachineSubsetCollectionNode::new(info_db.clone(), selector, SharedString::from(text));
				Rc::new(node) as Rc<dyn CollectionNode>
			})
			.collect()
	}
}

impl CollectionNode for MachineSubsetCollectionNode {
	fn text(&self) -> SharedString {
		self.text.clone()
	}

	fn get_items(&self) -> Rc<[Item]> {
		items_from_machines(&self.info_db, |machine| (self.selector)(machine) == self.text.as_ref())
	}
}
