use std::borrow::Cow;
use std::rc::Rc;

use crate::prefs::BuiltinCollection;
use crate::prefs::PrefsCollection;
use crate::prefs::PrefsItem;

pub fn get_folder_collections(collections: &[Rc<PrefsCollection>]) -> Vec<(usize, Rc<PrefsCollection>)> {
	collections
		.iter()
		.enumerate()
		.filter(|(_, col)| matches!(***col, PrefsCollection::Folder { .. }))
		.map(|(index, col)| (index, col.clone()))
		.collect()
}

pub fn get_folder_collection_names(collections: &[Rc<PrefsCollection>]) -> Vec<String> {
	collections
		.iter()
		.filter_map(|col| match &**col {
			PrefsCollection::Folder { name, .. } => Some(name.clone()),
			_ => None,
		})
		.collect()
}

pub fn add_items_to_new_folder_collection(
	collections: &mut Vec<Rc<PrefsCollection>>,
	name: String,
	items: Vec<PrefsItem>,
) {
	let col = PrefsCollection::Folder { name, items };
	let col = Rc::new(col);
	collections.push(col);
}

pub fn add_items_to_existing_folder_collection(
	collections: &mut [Rc<PrefsCollection>],
	folder_index: usize,
	mut new_items: Vec<PrefsItem>,
) {
	let mut col = Rc::unwrap_or_clone(collections[folder_index].clone());
	let PrefsCollection::Folder { items, .. } = &mut col else {
		panic!("Expected PrefsCollection::Folder");
	};

	new_items.retain(|x| !items.contains(x));
	items.extend(new_items);

	collections[folder_index] = Rc::new(col);
}

pub fn remove_items_from_folder_collection(
	collections: &mut [Rc<PrefsCollection>],
	folder_name: String,
	items: &[PrefsItem],
) {
	let (index, old_items) = collections
		.iter()
		.enumerate()
		.filter_map(|(index, col)| {
			if let PrefsCollection::Folder { name, items } = col.as_ref() {
				(name == &folder_name).then_some((index, items))
			} else {
				None
			}
		})
		.next()
		.unwrap();

	let new_items = old_items
		.iter()
		.filter(|x| !items.contains(x))
		.cloned()
		.collect::<Vec<_>>();

	let new_collection = PrefsCollection::Folder {
		name: folder_name,
		items: new_items,
	};
	collections[index] = Rc::new(new_collection);
}

pub fn toggle_builtin_collection(collections: &mut Vec<Rc<PrefsCollection>>, builtin: BuiltinCollection) {
	let old_len = collections.len();
	collections.retain(|x| !matches!(&**x, PrefsCollection::Builtin(x) if *x == builtin));

	if collections.len() == old_len {
		let new_collection = Rc::new(PrefsCollection::Builtin(builtin));
		collections.push(new_collection);
	}
}

pub fn get_collection_name(collections: &[Rc<PrefsCollection>], index: usize) -> Cow<'_, String> {
	match collections[index].as_ref() {
		PrefsCollection::Folder { name, .. } => Cow::Borrowed(name),
		PrefsCollection::Builtin(x) => Cow::Owned(format!("{}", x)),
		PrefsCollection::MachineSoftware { machine_name } => Cow::Borrowed(machine_name),
	}
}
