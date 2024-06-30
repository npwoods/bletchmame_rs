//! Wrapper model for implementing something that approximates a Tree Control, handling selection and item expansion
//!
//! Looking forward to Slint having a "real" tree control
//!
//! Terminology note:
//!     `index` - Represents an absolute index into the `entries` vector
//!     `row`   - Represents the visible row
use std::any::Any;
use std::cell::Cell;
use std::cell::RefCell;

use itertools::Itertools;
use slint::Model;
use slint::ModelNotify;
use slint::ModelTracker;

use crate::ui::TreeNode;

type SelectedItemChangedCallback<T> = Box<dyn Fn(&TreeModel<T>) + 'static>;

pub struct TreeModel<T> {
	pub entries: RefCell<Vec<Entry<T>>>,
	visibility: RefCell<Vec<usize>>,
	selected_index: Cell<Option<usize>>,

	notify: ModelNotify,
	selected_item_changed_callback: RefCell<Option<SelectedItemChangedCallback<T>>>,
}

pub struct Entry<T> {
	pub display: TreeNode,
	pub data: T,
	row: Option<usize>,
}

impl<T> TreeModel<T> {
	pub fn new() -> Self {
		Self {
			entries: Default::default(),
			visibility: Default::default(),
			selected_index: Default::default(),
			notify: Default::default(),
			selected_item_changed_callback: Default::default(),
		}
	}

	pub fn load(&self, entries: impl IntoIterator<Item = (TreeNode, T)>) {
		// build all entries
		let entries = entries
			.into_iter()
			.map(|(display, data)| Entry {
				display,
				data,
				row: None,
			})
			.collect();
		self.entries.replace(entries);

		self.notify.reset();

		// identify the selection
		let index = identify_selected_index(&mut self.entries.borrow_mut());
		self.set_selected_index(index);

		// update visibility in response
		self.update_visibility();
	}

	pub fn with_selected_data<R>(&self, func: impl FnOnce(&T) -> R) -> Option<R> {
		let entries = self.entries.borrow();
		self.selected_index
			.get()
			.and_then(|x| entries.get(x))
			.map(|entry| func(&entry.data))
	}

	pub fn on_selected_item_changed(&self, callback: impl Fn(&TreeModel<T>) + 'static) {
		self.selected_item_changed_callback.replace(Some(Box::new(callback)));
	}

	pub fn selected_row(&self) -> Option<usize> {
		let index = self.selected_index.get()?;
		self.entries.borrow().get(index).and_then(|e| e.row)
	}

	fn update_visibility(&self) {
		// inspect to get the new state
		let visibility = build_visibility(&mut self.entries.borrow_mut());

		// change the visibility
		self.visibility.replace(visibility);
		self.notify.reset();
	}

	pub fn has_selection(&self) -> bool {
		self.selected_index.get().is_some()
	}

	pub fn set_selected_index(&self, new_selected_index: Option<usize>) {
		// set the selected index entry and identify which indexes changed
		let changes = set_selected_index(&mut self.entries.borrow_mut(), &self.selected_index, new_selected_index);

		// notify callers that they changed
		match &changes {
			SetSelectedIndexChanges::Indexes(indexes) => {
				for row in indexes.iter().filter_map(|&index| self.entries.borrow()[index].row) {
					self.notify.row_changed(row);
				}
			}
			SetSelectedIndexChanges::Opened => self.update_visibility(),
		}

		// invoke selected_item_changed_callback if appropriate
		if !matches!(&changes, SetSelectedIndexChanges::Indexes(ref x) if x.is_empty()) {
			if let Some(selected_item_changed_callback) = self.selected_item_changed_callback.borrow().as_ref() {
				selected_item_changed_callback(self);
			}
		}
	}

	fn display_data(&self, index: usize) -> Option<TreeNode> {
		self.entries.borrow().get(index).map(|entry| entry.display.clone())
	}

	fn set_display_data(&self, index: usize, data: TreeNode) {
		let (selected_delta, open_delta) = {
			let mut entries = self.entries.borrow_mut();

			// determine deltas
			let selected_delta = (entries[index].display.is_selected != data.is_selected).then_some(data.is_selected);
			let open_delta = (entries[index].display.is_open != data.is_open).then_some(data.is_open);

			// update the row and return the deltas
			entries[index].display = data;
			(selected_delta, open_delta)
		};

		if let Some(new_is_selected) = selected_delta {
			let new_selected_index = new_is_selected.then_some(index);
			self.set_selected_index(new_selected_index)
		}

		if open_delta.is_some() {
			self.update_visibility();

			let new_selected_index = ensure_visible_selection(&self.entries.borrow(), self.selected_index.get());
			if let Some(new_selected_index) = new_selected_index {
				self.set_selected_index(new_selected_index);
			}
		}
	}
}

impl<T> TreeModel<T>
where
	T: Clone,
{
	pub fn selected_data(&self) -> Option<T> {
		self.with_selected_data(|x| x.clone())
	}
}

impl<T> Model for TreeModel<T>
where
	T: 'static,
{
	type Data = TreeNode;

	fn row_count(&self) -> usize {
		self.visibility.borrow().len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		self.visibility
			.borrow()
			.get(row)
			.and_then(|&row| self.display_data(row))
	}

	fn set_row_data(&self, row: usize, data: Self::Data) {
		let index = self.visibility.borrow().get(row).cloned();
		if let Some(index) = index {
			self.set_display_data(index, data);
		}
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

/// builds the visibility vector, and updates the `visibility_idx` on all entries
fn build_visibility<T>(entries: &mut [Entry<T>]) -> Vec<usize> {
	// build the visibility vector
	let visibility = entries
		.iter()
		.enumerate()
		.coalesce(|a, b| {
			let (_, a_entry) = &a;
			let (_, b_entry) = &b;
			if !a_entry.display.is_open && a_entry.display.indentation < b_entry.display.indentation {
				Ok(a)
			} else {
				Err((a, b))
			}
		})
		.map(|(idx, _)| idx)
		.collect::<Vec<_>>();

	// and update all `row` on all entries
	let mut visibility_iter = visibility.iter().enumerate().peekable();
	for (index, entry) in entries.iter_mut().enumerate() {
		entry.row = visibility_iter.next_if(|(_, &x)| x == index).map(|(row, _)| row);
	}

	visibility
}

fn identify_selected_index<T>(entries: &mut [Entry<T>]) -> Option<usize> {
	let selected_index = entries
		.iter()
		.enumerate()
		.filter_map(|(idx, entry)| entry.display.is_selected.then_some(idx))
		.next();

	if let Some(selected_index) = selected_index {
		for (index, entry) in entries.iter_mut().enumerate() {
			entry.display.is_selected = index == selected_index;
		}
	}

	selected_index
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SetSelectedIndexChanges {
	Indexes(Vec<usize>),
	Opened,
}

fn set_selected_index<T>(
	entries: &mut [Entry<T>],
	selected_index: &Cell<Option<usize>>,
	new_selected_index: Option<usize>,
) -> SetSelectedIndexChanges {
	// sanity checks
	assert!(!selected_index.get().is_some_and(|x| x >= entries.len()));
	assert!(!new_selected_index.is_some_and(|x| x >= entries.len()));

	let mut result = Vec::new();
	let mut items_opened = false;
	if selected_index.get() != new_selected_index {
		// identify the items that are different and might need to be updated
		if let Some(idx) = selected_index.get() {
			entries[idx].display.is_selected = false;
			result.push(idx);
		}
		if let Some(idx) = new_selected_index {
			entries[idx].display.is_selected = true;
			result.push(idx);
		}

		// specify the selection
		selected_index.set(new_selected_index);

		// ensure that all parents are open
		if let Some(new_selected_index) = new_selected_index {
			for entry in &mut entries[0..=new_selected_index]
				.iter_mut()
				.rev()
				.dedup_by(|a, b| a.display.indentation <= b.display.indentation)
				.skip(1)
				.filter(|entry| !entry.display.is_open)
			{
				entry.display.is_open = true;
				items_opened = true
			}
		}
	}

	if items_opened {
		SetSelectedIndexChanges::Opened
	} else {
		SetSelectedIndexChanges::Indexes(result)
	}
}

fn ensure_visible_selection<T>(entries: &[Entry<T>], selected_index: Option<usize>) -> Option<Option<usize>> {
	// nothing to do if we have no selection
	let selected_index = selected_index?;

	// we need to go "up" the indentation
	let new_selected_index = (0..=selected_index)
		.rev()
		.dedup_by(|&a, &b| entries[a].display.indentation <= entries[b].display.indentation)
		.find(|&x| entries[x].row.is_some());

	// only return something if the selection changed
	(Some(selected_index) != new_selected_index).then_some(new_selected_index)
}

#[cfg(test)]
mod test {
	use std::cell::Cell;

	use test_case::test_case;

	use crate::ui::TreeNode;

	use super::Entry;
	use super::SetSelectedIndexChanges;

	#[test_case(00, None, Some(1), SetSelectedIndexChanges::Indexes([1].into()), &[])]
	#[test_case(01, None, Some(2), SetSelectedIndexChanges::Indexes([2].into()), &[])]
	#[test_case(02, None, Some(3), SetSelectedIndexChanges::Opened, &[2])]
	#[test_case(02, None, Some(4), SetSelectedIndexChanges::Opened, &[2])]
	#[test_case(03, None, Some(7), SetSelectedIndexChanges::Indexes([7].into()), &[])]
	pub fn set_selected_index(
		_index: usize,
		selected_index: Option<usize>,
		new_selected_index: Option<usize>,
		expected: SetSelectedIndexChanges,
		expected_newly_opened: &[usize],
	) {
		let original_entries = [
			(0, false, Some(0)), //  [0]
			(0, true, Some(1)),  //  [1]
			(1, false, Some(2)), //     [2]
			(2, false, None),    //        ?3?
			(2, false, None),    //        ?4?
			(1, false, None),    //     ?5?
			(2, false, None),    //        ?6?
			(0, true, Some(5)),  //  [7]
			(1, false, Some(6)), //     [8]
			(2, false, None),    //        ?9?
		];
		let mut entries = original_entries
			.iter()
			.cloned()
			.map(|(indentation, is_open, row)| Entry {
				display: TreeNode {
					indentation,
					is_open,
					..Default::default()
				},
				data: (),
				row,
			})
			.collect::<Vec<_>>();

		let selected_index = Cell::new(selected_index);
		let actual = super::set_selected_index(&mut entries, &selected_index, new_selected_index);
		assert_eq!(expected, actual);

		// validate which ones were opened
		let actual_newly_opened = Iterator::zip(original_entries.iter(), entries.iter())
			.enumerate()
			.filter_map(|(index, ((_, is_opened_before, _), entry))| {
				let is_opened_after = entry.display.is_open;
				(!is_opened_before && is_opened_after).then_some(index)
			})
			.collect::<Vec<_>>();
		assert_eq!(expected_newly_opened, actual_newly_opened.as_slice());
	}

	#[test_case(0, None, None)]
	#[test_case(1, Some(1), None)]
	#[test_case(2, Some(2), None)]
	#[test_case(3, Some(4), Some(2))]
	#[test_case(4, Some(5), Some(1))]
	#[test_case(5, Some(6), Some(1))]
	#[test_case(6, Some(8), None)]
	#[test_case(7, Some(9), Some(8))]
	pub fn ensure_visible_selection(_index: usize, selected_index: Option<usize>, expected: Option<usize>) {
		let entries = [
			(0, Some(0)), //  [0]
			(0, Some(1)), //  [1]
			(1, Some(2)), //     [2]
			(2, None),    //        ?3?
			(2, None),    //        ?4?
			(1, None),    //     ?5?
			(2, None),    //        ?6?
			(0, Some(5)), //  [7]
			(1, Some(6)), //     [8]
			(2, None),    //        ?9?
		];
		let entries = entries
			.into_iter()
			.map(|(indentation, row)| Entry {
				display: TreeNode {
					indentation,
					..Default::default()
				},
				data: (),
				row,
			})
			.collect::<Vec<_>>();

		let expected = expected.map(|x| Some(x));
		let actual = super::ensure_visible_selection(&entries, selected_index);
		assert_eq!(expected, actual);
	}
}
