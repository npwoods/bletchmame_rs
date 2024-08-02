use std::rc::Rc;

use crate::prefs::HistoryEntry;
use crate::prefs::Preferences;
use crate::prefs::PrefsCollection;

const MAX_HISTORY_LEN: usize = 10;

pub trait History {
	fn history_push(&mut self, collection: Rc<PrefsCollection>);
	fn history_advance(&mut self, delta: isize);
	fn can_history_advance(&self, delta: isize) -> bool;
	fn current_collection(&self) -> (Rc<PrefsCollection>, Option<usize>);
	fn current_history_entry(&self) -> &HistoryEntry;
	fn current_history_entry_mut(&mut self) -> &mut HistoryEntry;
}

pub trait HistoryContainer {
	fn entries(&self) -> (&[HistoryEntry], usize);
	fn entries_mut(&mut self) -> (&mut Vec<HistoryEntry>, &mut usize);
	fn collections(&self) -> &[Rc<PrefsCollection>];
}

impl<T> History for T
where
	T: HistoryContainer,
{
	fn history_push(&mut self, collection: Rc<PrefsCollection>) {
		let (history, position) = self.entries_mut();

		let history_entry: HistoryEntry = HistoryEntry {
			collection: sanitize_collection(collection),
			search: "".into(),
			selection: Vec::default(),
		};

		history.truncate(history.len().saturating_sub(*position));
		history.push(history_entry);
		*position = 0;
		if history.len() > MAX_HISTORY_LEN {
			history.drain(..(history.len() - MAX_HISTORY_LEN));
		}
	}

	fn history_advance(&mut self, delta: isize) {
		let (history, position) = self.entries_mut();
		*position = advance_position(*position, history.len(), delta).unwrap();
	}

	fn can_history_advance(&self, delta: isize) -> bool {
		let (history, position) = self.entries();
		advance_position(position, history.len(), delta).is_some()
	}

	fn current_collection(&self) -> (Rc<PrefsCollection>, Option<usize>) {
		let collections = self.collections();
		let target_collection = &self.current_history_entry().collection;

		let (collection, collection_index) = if let Some((index, collection)) = collections
			.iter()
			.enumerate()
			.find(|(_, collection)| &sanitize_collection((*collection).clone()) == target_collection)
		{
			(collection, Some(index))
		} else {
			(target_collection, None)
		};
		(collection.clone(), collection_index)
	}

	fn current_history_entry(&self) -> &'_ HistoryEntry {
		let (history, position) = self.entries();
		&history[history.len() - position - 1]
	}

	fn current_history_entry_mut(&mut self) -> &mut HistoryEntry {
		let (history, position) = self.entries_mut();
		let history_len = history.len();
		&mut history[history_len - *position - 1]
	}
}

fn advance_position(position: usize, length: usize, delta: isize) -> Option<usize> {
	assert!(position < length);
	let position = position.wrapping_add_signed(-delta);
	(position < length).then_some(position)
}

fn sanitize_collection(collection: Rc<PrefsCollection>) -> Rc<PrefsCollection> {
	if let PrefsCollection::Folder { name, items: _ } = collection.as_ref() {
		let name = name.clone();
		let collection = PrefsCollection::Folder {
			name,
			items: Vec::default(),
		};
		Rc::new(collection)
	} else {
		collection
	}
}

impl HistoryContainer for Preferences {
	fn entries(&self) -> (&[HistoryEntry], usize) {
		(&self.history, self.history_position)
	}

	fn entries_mut(&mut self) -> (&mut Vec<HistoryEntry>, &mut usize) {
		(&mut self.history, &mut self.history_position)
	}

	fn collections(&self) -> &[Rc<PrefsCollection>] {
		&self.collections
	}
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	#[test_case(0, 0, 3, 1, None)]
	#[test_case(1, 0, 3, -1, Some(1))]
	#[test_case(2, 1, 3, 1, Some(0))]
	#[test_case(3, 1, 3, -1, Some(2))]
	#[test_case(4, 2, 3, 1, Some(1))]
	#[test_case(5, 2, 3, -1, None)]
	pub fn advance_position(_index: usize, position: usize, length: usize, delta: isize, expected: Option<usize>) {
		let actual = super::advance_position(position, length, delta);
		assert_eq!(expected, actual);
	}
}
