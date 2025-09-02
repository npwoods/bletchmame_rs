use serde::Deserialize;
use strum::EnumString;
use zerocopy::Immutable;
use zerocopy::IntoBytes;
use zerocopy::KnownLayout;
use zerocopy::TryFromBytes;
use zerocopy::little_endian::U16;
use zerocopy::little_endian::U32;
use zerocopy::little_endian::U64;

use crate::info::UsizeDb;

pub trait Fixup {
	fn identify_machine_indexes(&mut self) -> impl IntoIterator<Item = &mut UsizeDb> {
		[]
	}
	fn identify_software_list_indexes(&mut self) -> impl IntoIterator<Item = &mut UsizeDb> {
		[]
	}
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Header {
	pub magic: [u8; 8],
	pub serial: U16,
	pub sizes_hash: U64,
	pub build_strindex: UsizeDb,
	pub machine_count: UsizeDb,
	pub biosset_count: UsizeDb,
	pub chips_count: UsizeDb,
	pub config_count: UsizeDb,
	pub config_setting_count: UsizeDb,
	pub config_setting_condition_count: UsizeDb,
	pub device_count: UsizeDb,
	pub slot_count: UsizeDb,
	pub slot_option_count: UsizeDb,
	pub software_list_count: UsizeDb,
	pub software_list_machine_count: UsizeDb,
	pub machine_software_lists_count: UsizeDb,
	pub ram_option_count: UsizeDb,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Machine {
	pub name_strindex: UsizeDb,
	pub source_file_strindex: UsizeDb,
	pub clone_of_machine_index: UsizeDb,
	pub rom_of_machine_index: UsizeDb,
	pub description_strindex: UsizeDb,
	pub year_strindex: UsizeDb,
	pub manufacturer_strindex: UsizeDb,
	pub biossets_start: UsizeDb,
	pub biossets_end: UsizeDb,
	pub default_biosset_index: UsizeDb,
	pub chips_start: UsizeDb,
	pub chips_end: UsizeDb,
	pub configs_start: UsizeDb,
	pub configs_end: UsizeDb,
	pub devices_start: UsizeDb,
	pub devices_end: UsizeDb,
	pub slots_start: UsizeDb,
	pub slots_end: UsizeDb,
	pub slot_options_start: UsizeDb,
	pub slot_options_end: UsizeDb,
	pub machine_software_lists_start: UsizeDb,
	pub machine_software_lists_end: UsizeDb,
	pub ram_options_start: UsizeDb,
	pub ram_options_end: UsizeDb,
	pub runnable: bool,
}

impl Fixup for Machine {
	fn identify_machine_indexes(&mut self) -> impl IntoIterator<Item = &mut UsizeDb> {
		[&mut self.clone_of_machine_index, &mut self.rom_of_machine_index]
	}
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BiosSet {
	pub name_strindex: UsizeDb,
	pub description_strindex: UsizeDb,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Chip {
	pub clock: U64,
	pub tag_strindex: UsizeDb,
	pub name_strindex: UsizeDb,
	pub chip_type: ChipType,
}

#[repr(u8)]
#[derive(
	Clone, Copy, Debug, Deserialize, TryFromBytes, IntoBytes, Immutable, KnownLayout, EnumString, PartialEq, Eq,
)]
pub enum ChipType {
	#[strum(serialize = "cpu")]
	Cpu,
	#[strum(serialize = "audio")]
	Audio,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout, PartialEq)]
pub struct Configuration {
	pub name_strindex: UsizeDb,
	pub tag_strindex: UsizeDb,
	pub mask: U32,
	pub settings_start: UsizeDb,
	pub settings_end: UsizeDb,
	pub default_setting_index: UsizeDb,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout, PartialEq)]
pub struct ConfigurationSetting {
	pub name_strindex: UsizeDb,
	pub value: U32,
	pub conditions_start: UsizeDb,
	pub conditions_end: UsizeDb,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout, PartialEq)]
pub struct ConfigurationSettingCondition {
	pub tag_strindex: UsizeDb,
	pub condition_relation: ConditionRelation,
	pub mask: U32,
	pub value: U32,
}

#[repr(u8)]
#[derive(
	Clone, Copy, Debug, Deserialize, TryFromBytes, IntoBytes, Immutable, KnownLayout, EnumString, PartialEq, Eq,
)]
pub enum ConditionRelation {
	#[strum(serialize = "eq")]
	Eq,
	#[strum(serialize = "ne")]
	Ne,
	#[strum(serialize = "gt")]
	Gt,
	#[strum(serialize = "le")]
	Le,
	#[strum(serialize = "lt")]
	Lt,
	#[strum(serialize = "ge")]
	Ge,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Device {
	pub type_strindex: UsizeDb,
	pub tag_strindex: UsizeDb,
	pub mandatory: bool,
	pub interfaces_strindex: UsizeDb,
	pub extensions_strindex: UsizeDb,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Slot {
	pub name_strindex: UsizeDb,
	pub options_start: UsizeDb,
	pub options_end: UsizeDb,
	pub default_option_index: UsizeDb,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SlotOption {
	pub name_strindex: UsizeDb,
	pub devname_strindex: UsizeDb,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MachineSoftwareList {
	pub tag_strindex: UsizeDb,
	pub software_list_index: UsizeDb,
	pub status: SoftwareListStatus,
	pub filter_strindex: UsizeDb,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct RamOption {
	pub size: U64,
	pub is_default: bool,
}

impl Fixup for MachineSoftwareList {
	fn identify_software_list_indexes(&mut self) -> impl IntoIterator<Item = &mut UsizeDb> {
		[&mut self.software_list_index]
	}
}

#[repr(u8)]
#[derive(
	Clone, Copy, Debug, Deserialize, TryFromBytes, IntoBytes, Immutable, KnownLayout, EnumString, PartialEq, Eq,
)]
pub enum SoftwareListStatus {
	#[strum(serialize = "original")]
	Original,
	#[strum(serialize = "compatible")]
	Compatible,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SoftwareList {
	pub name_strindex: UsizeDb,
	pub software_list_original_machines_start: UsizeDb,
	pub software_list_compatible_machines_start: UsizeDb,
	pub software_list_compatible_machines_end: UsizeDb,
}

#[cfg(test)]
mod test {
	use std::str::FromStr;

	use test_case::test_case;

	use super::ChipType;

	#[test_case(0, "cpu", Ok(ChipType::Cpu))]
	#[test_case(1, "audio", Ok(ChipType::Audio))]
	#[test_case(2, "<<invalid>>", Err(()))]
	pub fn chip_type_from_str(_index: usize, s: &str, expected: std::result::Result<ChipType, ()>) {
		let actual = ChipType::from_str(s).map_err(|_| ());
		assert_eq!(expected, actual);
	}
}
