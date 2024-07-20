use std::cmp::min;
use std::rc::Rc;

use crate::prefs::HistoryEntry;
use crate::prefs::PrefsCollection;

const MAX_HISTORY_LEN: usize = 10;

pub fn history_push(history: &mut Vec<HistoryEntry>, history_position: &mut usize, collection: &Rc<PrefsCollection>) {
	let history_entry: HistoryEntry = HistoryEntry {
		collection: sanitize_collection(collection),
		selection: Vec::default(),
	};

	history.truncate(history.len().saturating_sub(*history_position));
	history.push(history_entry);
	*history_position = 0;
	if history.len() > MAX_HISTORY_LEN {
		history.drain(..(history.len() - MAX_HISTORY_LEN));
	}
}

pub fn history_advance(history: &mut Vec<HistoryEntry>, history_position: &mut usize, delta: isize) {
	*history_position = min(
		history_position.saturating_add_signed(delta),
		history.len().saturating_sub(1),
	);
}

/// Given collections and history information, get the fully qualified collection
pub fn collection_for_current_history_item(
	collections: &[Rc<PrefsCollection>],
	history: &[HistoryEntry],
	history_position: usize,
) -> Option<(Rc<PrefsCollection>, Option<usize>)> {
	let target_collection = history
		.get(history.len() - history_position - 1)
		.map(|x: &HistoryEntry| &x.collection)?;

	let (collection, collection_index) = if let Some((index, collection)) = collections
		.iter()
		.enumerate()
		.find(|(_, collection)| &sanitize_collection(collection) == target_collection)
	{
		(collection, Some(index))
	} else {
		(target_collection, None)
	};
	Some((collection.clone(), collection_index))
}

fn sanitize_collection(collection: &Rc<PrefsCollection>) -> Rc<PrefsCollection> {
	if let PrefsCollection::Folder { name, items: _ } = collection.as_ref() {
		let name = name.clone();
		let collection = PrefsCollection::Folder {
			name,
			items: Vec::default(),
		};
		Rc::new(collection)
	} else {
		collection.clone()
	}
}

#[cfg(test)]
mod test {
	use std::rc::Rc;

	use test_case::test_case;

	use crate::prefs::BuiltinCollection;
	use crate::prefs::HistoryEntry;
	use crate::prefs::PrefsCollection;

	use super::history_advance;

	#[test_case(0, 2, -1, 1)]
	#[test_case(1, 2, 1, 3)]
	pub fn advance(_index: usize, mut history_position: usize, delta: isize, expected: usize) {
		let mut history = (0..4)
			.map(|_| HistoryEntry {
				collection: Rc::new(PrefsCollection::Builtin(BuiltinCollection::All)),
				selection: Vec::default(),
			})
			.collect::<Vec<_>>();

		history_advance(&mut history, &mut history_position, delta);
		assert_eq!(expected, history_position);
	}
}
