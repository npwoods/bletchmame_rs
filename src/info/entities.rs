use binary_search::binary_search;
use binary_search::Direction;

use crate::info::binary;
use crate::info::ChipType;
use crate::info::Object;
use crate::info::SmallStrRef;
use crate::info::View;

pub type Machine<'a> = Object<'a, binary::Machine>;
pub type MachinesView<'a> = View<'a, binary::Machine>;
pub type Chip<'a> = Object<'a, binary::Chip>;
pub type ChipsView<'a> = View<'a, binary::Chip>;
pub type SoftwareList<'a> = Object<'a, binary::SoftwareList>;
pub type SoftwareListsView<'a> = View<'a, binary::SoftwareList>;

impl<'a> Machine<'a> {
	pub fn name(&self) -> SmallStrRef<'a> {
		self.string(|x| x.name_strindex)
	}

	pub fn source_file(&self) -> SmallStrRef<'a> {
		self.string(|x| x.source_file_strindex)
	}

	pub fn description(&self) -> SmallStrRef<'a> {
		self.string(|x| x.description_strindex)
	}

	pub fn year(&self) -> SmallStrRef<'a> {
		self.string(|x| x.year_strindex)
	}

	pub fn manufacturer(&self) -> SmallStrRef<'a> {
		self.string(|x| x.manufacturer_strindex)
	}

	pub fn runnable(&self) -> bool {
		self.obj().runnable
	}

	pub fn chips(&self) -> ChipsView<'a> {
		self.db.chips().sub_view(self.obj().chips_index, self.obj().chips_count)
	}

	pub fn software_lists(&self) -> SoftwareListsView<'a> {
		self.db
			.software_lists()
			.sub_view(self.obj().software_lists_index, self.obj().software_lists_count)
	}
}

impl<'a> MachinesView<'a> {
	pub fn find_index(&self, target: &str) -> Option<usize> {
		if self.len() == 0 {
			return None;
		}

		let ((largest_low, _), _) = binary_search((0, ()), (self.len(), ()), |i| {
			if self.get(i).unwrap().name().as_ref() <= target {
				Direction::Low(())
			} else {
				Direction::High(())
			}
		});
		let machine = self.get(largest_low).unwrap();
		(machine.name() == target).then_some(largest_low)
	}

	pub fn find(&self, target: &str) -> Option<Machine<'a>> {
		self.find_index(target).map(|index| self.get(index).unwrap())
	}
}

impl<'a> Chip<'a> {
	pub fn tag(&self) -> SmallStrRef<'a> {
		self.string(|x| x.tag_strindex)
	}

	pub fn name(&self) -> SmallStrRef<'a> {
		self.string(|x| x.name_strindex)
	}

	pub fn chip_type(&self) -> ChipType {
		self.obj().chip_type
	}
}

#[cfg(test)]
mod test {
	use std::marker::PhantomData;

	use crate::info::InfoDb;

	use super::MachinesView;

	#[test]
	pub fn empty_machine_find() {
		let xml = include_str!("test_data/listxml_fake.xml");
		let bogus_db = InfoDb::from_listxml_output(xml.as_bytes(), |_| false).unwrap().unwrap();

		let machines_view = MachinesView {
			db: &bogus_db,
			offset: 0,
			count: 0,
			phantom: PhantomData,
		};

		let actual = machines_view.find("cant_find_this");
		assert!(actual.is_none());
	}
}
