use std::borrow::Cow;

use binary_search::binary_search;
use binary_search::Direction;

use crate::info::binary;
use crate::info::ChipType;
use crate::info::IndirectView;
use crate::info::Object;
use crate::info::SimpleView;
use crate::info::SmallStrRef;
use crate::info::View;

pub type Machine<'a> = Object<'a, binary::Machine>;
pub type MachinesView<'a> = SimpleView<'a, binary::Machine>;
pub type Chip<'a> = Object<'a, binary::Chip>;
pub type Device<'a> = Object<'a, binary::Device>;
pub type Slot<'a> = Object<'a, binary::Slot>;
pub type SlotOption<'a> = Object<'a, binary::SlotOption>;
pub type SoftwareList<'a> = Object<'a, binary::SoftwareList>;
pub type SoftwareListsView<'a> = SimpleView<'a, binary::SoftwareList>;
pub type MachineSoftwareList<'a> = Object<'a, binary::MachineSoftwareList>;

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

	pub fn clone_of(&self) -> Option<Machine<'a>> {
		let clone_of_machine_index = self.obj().clone_of_machine_index.try_into().unwrap();
		self.db.machines().get(clone_of_machine_index)
	}

	pub fn rom_of(&self) -> Option<Machine<'a>> {
		let rom_of_machine_index = self.obj().rom_of_machine_index.try_into().unwrap();
		self.db.machines().get(rom_of_machine_index)
	}

	pub fn runnable(&self) -> bool {
		self.obj().runnable
	}

	pub fn chips(&self) -> impl View<'a, Chip<'a>> {
		self.db.chips().sub_view(self.obj().chips_start..self.obj().chips_end)
	}

	pub fn devices(&self) -> impl View<'a, Device<'a>> {
		self.db
			.devices()
			.sub_view(self.obj().devices_start..self.obj().devices_end)
	}

	pub fn slots(&self) -> impl View<'a, Slot<'a>> {
		self.db.slots().sub_view(self.obj().slots_start..self.obj().slots_end)
	}

	pub fn machine_software_lists(&self) -> impl View<'a, MachineSoftwareList<'a>> {
		self.db
			.machine_software_lists()
			.sub_view(self.obj().machine_software_lists_start..self.obj().machine_software_lists_end)
	}
}

impl<'a> MachinesView<'a> {
	pub fn find_index(&self, target: &str) -> Option<usize> {
		if self.is_empty() {
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

impl<'a> Device<'a> {
	pub fn device_type(&self) -> SmallStrRef<'a> {
		self.string(|x| x.type_strindex)
	}

	pub fn tag(&self) -> SmallStrRef<'a> {
		self.string(|x| x.tag_strindex)
	}

	pub fn mandatory(&self) -> bool {
		self.obj().mandatory
	}

	pub fn interface(&self) -> SmallStrRef<'a> {
		self.string(|x| x.interface_strindex)
	}

	pub fn extensions(&self) -> impl Iterator<Item = Cow<'a, str>> {
		self.string(|x| x.extensions_strindex).split('\0')
	}
}

impl<'a> Slot<'a> {
	pub fn name(&self) -> SmallStrRef<'a> {
		self.string(|x| x.name_strindex)
	}

	pub fn options(&self) -> impl View<'a, SlotOption<'a>> {
		self.db
			.slot_options()
			.sub_view(self.obj().options_start..self.obj().options_end)
	}

	pub fn default_option_index(&self) -> Option<usize> {
		let index = self.obj().default_option_index as usize;
		(index < self.options().len()).then_some(index)
	}
}

impl<'a> SlotOption<'a> {
	pub fn name(&self) -> SmallStrRef<'a> {
		self.string(|x| x.name_strindex)
	}

	pub fn devname(&self) -> SmallStrRef<'a> {
		self.string(|x| x.devname_strindex)
	}
}

impl<'a> SoftwareList<'a> {
	pub fn name(&self) -> SmallStrRef<'a> {
		self.string(|x| x.name_strindex)
	}

	pub fn original_for_machines(&self) -> impl View<'a, Machine<'a>> {
		let start = self.obj().software_list_original_machines_start;
		let end = self.obj().software_list_compatible_machines_start;
		self.make_machine_view(start, end)
	}

	pub fn compatible_for_machines(&self) -> impl View<'a, Machine<'a>> {
		let start = self.obj().software_list_compatible_machines_start;
		let end = self.obj().software_list_compatible_machines_end;
		self.make_machine_view(start, end)
	}

	fn make_machine_view(&self, start: u32, end: u32) -> impl View<'a, Machine<'a>> {
		let range = start..end;
		let index_view = self.db.software_list_machine_indexes().sub_view(range);
		let object_view = self.db.machines();
		IndirectView {
			index_view,
			object_view,
		}
	}
}

impl<'a> SoftwareListsView<'a> {
	pub fn find(&self, target: &str) -> Option<SoftwareList<'a>> {
		self.iter().find(|x| x.name() == target)
	}
}

impl<'a> MachineSoftwareList<'a> {
	pub fn tag(&self) -> SmallStrRef<'a> {
		self.string(|x| x.tag_strindex)
	}

	pub fn software_list(&self) -> SoftwareList<'a> {
		let software_list_index = self.obj().software_list_index.try_into().unwrap();
		self.db.software_lists().get(software_list_index).unwrap()
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
			byte_offset: bogus_db.machines().byte_offset,
			start: 0,
			end: 0,
			phantom: PhantomData,
		};

		let actual = machines_view.find("cant_find_this");
		assert!(actual.is_none());
	}
}
