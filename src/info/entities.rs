#![allow(dead_code)]
use anyhow::Result;
use anyhow::ensure;
use binary_search::Direction;
use binary_search::binary_search;

use crate::info::ChipType;
use crate::info::ConditionRelation;
use crate::info::IndirectView;
use crate::info::Object;
use crate::info::SimpleView;
use crate::info::Validatable;
use crate::info::View;
use crate::info::binary;

pub type Machine<'a> = Object<'a, binary::Machine>;
pub type MachinesView<'a> = SimpleView<'a, binary::Machine>;
pub type BiosSet<'a> = Object<'a, binary::BiosSet>;
pub type Chip<'a> = Object<'a, binary::Chip>;
pub type Configuration<'a> = Object<'a, binary::Configuration>;
pub type ConfigurationSetting<'a> = Object<'a, binary::ConfigurationSetting>;
pub type ConfigurationSettingCondition<'a> = Object<'a, binary::ConfigurationSettingCondition>;
pub type Device<'a> = Object<'a, binary::Device>;
pub type Slot<'a> = Object<'a, binary::Slot>;
pub type SlotOption<'a> = Object<'a, binary::SlotOption>;
pub type SoftwareList<'a> = Object<'a, binary::SoftwareList>;
pub type SoftwareListsView<'a> = SimpleView<'a, binary::SoftwareList>;
pub type MachineSoftwareList<'a> = Object<'a, binary::MachineSoftwareList>;
pub type RamOption<'a> = Object<'a, binary::RamOption>;

impl<'a> Machine<'a> {
	pub fn name(&self) -> &'a str {
		self.string(|x| x.name_strindex)
	}

	pub fn source_file(&self) -> &'a str {
		self.string(|x| x.source_file_strindex)
	}

	pub fn description(&self) -> &'a str {
		self.string(|x| x.description_strindex)
	}

	pub fn year(&self) -> &'a str {
		self.string(|x| x.year_strindex)
	}

	pub fn manufacturer(&self) -> &'a str {
		self.string(|x| x.manufacturer_strindex)
	}

	pub fn clone_of(&self) -> Option<Machine<'a>> {
		let clone_of_machine_index = self.obj().clone_of_machine_index.into();
		self.db.machines().get(clone_of_machine_index)
	}

	pub fn rom_of(&self) -> Option<Machine<'a>> {
		let rom_of_machine_index = self.obj().rom_of_machine_index.into();
		self.db.machines().get(rom_of_machine_index)
	}

	pub fn runnable(&self) -> bool {
		self.obj().runnable
	}

	pub fn biossets(&self) -> impl View<'a, BiosSet<'a>> + use<'a> {
		let range = self.obj().biossets_start.into()..self.obj().biossets_end.into();
		self.db.biossets().sub_view(range)
	}

	pub fn default_biosset_index(&self) -> Option<usize> {
		(!self.biossets().is_empty()).then(|| {
			let default_biosset_index = self.obj().default_biosset_index.into();
			if default_biosset_index < self.biossets().len() {
				default_biosset_index
			} else {
				0
			}
		})
	}

	pub fn chips(&self) -> impl View<'a, Chip<'a>> + use<'a> {
		let range = self.obj().chips_start.into()..self.obj().chips_end.into();
		self.db.chips().sub_view(range)
	}

	pub fn configurations(&self) -> impl View<'a, Configuration<'a>> + use<'a> {
		let range = self.obj().configs_start.into()..self.obj().configs_end.into();
		self.db.configurations().sub_view(range)
	}

	pub fn devices(&self) -> impl View<'a, Device<'a>> + use<'a> {
		let range = self.obj().devices_start.into()..self.obj().devices_end.into();
		self.db.devices().sub_view(range)
	}

	pub fn slots(&self) -> impl View<'a, Slot<'a>> + use<'a> {
		let range = self.obj().slots_start.into()..self.obj().slots_end.into();
		self.db.slots().sub_view(range)
	}

	pub fn machine_software_lists(&self) -> impl View<'a, MachineSoftwareList<'a>> + use<'a> {
		let range = self.obj().machine_software_lists_start.into()..self.obj().machine_software_lists_end.into();
		self.db.machine_software_lists().sub_view(range)
	}

	pub fn ram_options(&self) -> impl View<'a, RamOption<'a>> + use<'a> {
		let range = self.obj().ram_options_start.into()..self.obj().ram_options_end.into();
		self.db.ram_options().sub_view(range)
	}

	pub fn default_ram_option_index(&self) -> Option<usize> {
		self.ram_options().iter().position(|x| x.is_default())
	}
}

impl<'a> MachinesView<'a> {
	pub fn find_index(&self, target: &str) -> Result<usize> {
		let result = if !self.is_empty() {
			let ((largest_low, _), _) = binary_search((0, ()), (self.len(), ()), |i| {
				if self.get(i).unwrap().name() <= target {
					Direction::Low(())
				} else {
					Direction::High(())
				}
			});
			let machine = self.get(largest_low).unwrap();
			(machine.name() == target).then_some(largest_low)
		} else {
			None
		};
		result.ok_or_else(|| ThisError::CannotFindMachine(target.to_string()).into())
	}

	pub fn find(&self, target: &str) -> Result<Machine<'a>> {
		self.find_index(target).map(|index| self.get(index).unwrap())
	}
}

impl<'a> BiosSet<'a> {
	pub fn name(&self) -> &'a str {
		self.string(|x| x.name_strindex)
	}

	pub fn description(&self) -> &'a str {
		self.string(|x| x.description_strindex)
	}
}

impl<'a> Chip<'a> {
	pub fn tag(&self) -> &'a str {
		self.string(|x| x.tag_strindex)
	}

	pub fn name(&self) -> &'a str {
		self.string(|x| x.name_strindex)
	}

	pub fn chip_type(&self) -> ChipType {
		self.obj().chip_type
	}
}

impl<'a> Configuration<'a> {
	pub fn name(&self) -> &'a str {
		self.string(|x| x.name_strindex)
	}

	pub fn tag(&self) -> &'a str {
		self.string(|x| x.tag_strindex)
	}

	pub fn mask(&self) -> u32 {
		self.obj().mask.into()
	}

	pub fn settings(&self) -> impl View<'a, ConfigurationSetting<'a>> + use<'a> {
		let range = self.obj().settings_start.into()..self.obj().settings_end.into();
		self.db.configuration_settings().sub_view(range)
	}

	pub fn default_setting_index(&self) -> Option<usize> {
		let default_setting_index = self.obj().default_setting_index.into();
		(default_setting_index < self.settings().len()).then_some(default_setting_index)
	}
}

impl<'a> ConfigurationSetting<'a> {
	pub fn name(&self) -> &'a str {
		self.string(|x| x.name_strindex)
	}

	pub fn value(&self) -> u32 {
		self.obj().value.into()
	}

	pub fn conditions(&self) -> impl View<'a, ConfigurationSettingCondition<'a>> + use<'a> {
		let range = self.obj().conditions_start.into()..self.obj().conditions_end.into();
		self.db.configuration_setting_conditions().sub_view(range)
	}
}

impl<'a> ConfigurationSettingCondition<'a> {
	pub fn tag(&self) -> &'a str {
		self.string(|x| x.tag_strindex)
	}

	pub fn relation(&self) -> ConditionRelation {
		self.obj().condition_relation
	}

	pub fn mask(&self) -> u32 {
		self.obj().mask.into()
	}

	pub fn value(&self) -> u32 {
		self.obj().value.into()
	}
}

impl<'a> Device<'a> {
	pub fn device_type(&self) -> &'a str {
		self.string(|x| x.type_strindex)
	}

	pub fn tag(&self) -> &'a str {
		self.string(|x| x.tag_strindex)
	}

	pub fn mandatory(&self) -> bool {
		self.obj().mandatory
	}

	pub fn interfaces(&self) -> impl Iterator<Item = &'a str> + use<'a> {
		self.string(|x| x.interfaces_strindex).split('\0')
	}

	pub fn extensions(&self) -> impl Iterator<Item = &'a str> + use<'a> {
		self.string(|x| x.extensions_strindex).split('\0')
	}
}

impl<'a> Slot<'a> {
	pub fn name(&self) -> &'a str {
		self.string(|x| x.name_strindex)
	}

	pub fn options(&self) -> impl View<'a, SlotOption<'a>> + use<'a> {
		let range = self.obj().options_start.into()..self.obj().options_end.into();
		self.db.slot_options().sub_view(range)
	}

	pub fn default_option_index(&self) -> Option<usize> {
		let index = self.obj().default_option_index.into();
		(index < self.options().len()).then_some(index)
	}
}

impl<'a> SlotOption<'a> {
	pub fn name(&self) -> &'a str {
		self.string(|x| x.name_strindex)
	}

	pub fn devname(&self) -> &'a str {
		self.string(|x| x.devname_strindex)
	}
}

impl<'a> SoftwareList<'a> {
	pub fn name(&self) -> &'a str {
		self.string(|x| x.name_strindex)
	}

	pub fn original_for_machines(&self) -> impl View<'a, Machine<'a>> + use<'a> {
		let start = self.obj().software_list_original_machines_start.into();
		let end = self.obj().software_list_compatible_machines_start.into();
		self.make_machine_view(start, end)
	}

	pub fn compatible_for_machines(&self) -> impl View<'a, Machine<'a>> + use<'a> {
		let start = self.obj().software_list_compatible_machines_start.into();
		let end = self.obj().software_list_compatible_machines_end.into();
		self.make_machine_view(start, end)
	}

	fn make_machine_view(&self, start: usize, end: usize) -> impl View<'a, Machine<'a>> + use<'a> {
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
	pub fn find(&self, target: &str) -> Result<SoftwareList<'a>> {
		self.iter()
			.find(|x| x.name() == target)
			.ok_or_else(|| ThisError::CannotFindSoftwareList(target.to_string()).into())
	}
}

impl<'a> MachineSoftwareList<'a> {
	pub fn tag(&self) -> &'a str {
		self.string(|x| x.tag_strindex)
	}

	pub fn software_list(&self) -> SoftwareList<'a> {
		let software_list_index = self.obj().software_list_index.into();
		self.db.software_lists().get(software_list_index).unwrap()
	}
}

impl Validatable for MachineSoftwareList<'_> {
	fn validate(&self) -> Result<()> {
		let software_list_index = usize::from(self.obj().software_list_index);
		ensure!(software_list_index < self.db.software_lists().len());
		Ok(())
	}
}

impl RamOption<'_> {
	pub fn size(&self) -> u64 {
		self.obj().size.into()
	}

	pub fn is_default(&self) -> bool {
		self.obj().is_default
	}
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
enum ThisError {
	#[error("cannot find machine {0:?}")]
	CannotFindMachine(String),
	#[error("cannot find software list {0:?}")]
	CannotFindSoftwareList(String),
}

#[cfg(test)]
mod test {
	use super::MachinesView;
	use super::ThisError;

	#[test]
	pub fn empty_machine_find() {
		let actual = MachinesView::default()
			.find("cant_find_this")
			.map_err(|e| e.downcast().unwrap());
		assert_eq!(Err(ThisError::CannotFindMachine("cant_find_this".to_string())), actual);
	}
}
