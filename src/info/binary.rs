use serde::Deserialize;
use strum::EnumString;
use zerocopy::Immutable;
use zerocopy::IntoBytes;
use zerocopy::KnownLayout;
use zerocopy::TryFromBytes;
use zerocopy::little_endian::U64;

use crate::info::usize_db;

pub trait Fixup {
	fn identify_machine_indexes(&mut self) -> impl IntoIterator<Item = &mut usize_db> {
		[]
	}
	fn identify_software_list_indexes(&mut self) -> impl IntoIterator<Item = &mut usize_db> {
		[]
	}
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Header {
	pub magic: [u8; 8],
	pub sizes_hash: U64,
	pub build_strindex: usize_db,
	pub machine_count: usize_db,
	pub chips_count: usize_db,
	pub device_count: usize_db,
	pub slot_count: usize_db,
	pub slot_option_count: usize_db,
	pub software_list_count: usize_db,
	pub software_list_machine_count: usize_db,
	pub machine_software_lists_count: usize_db,
	pub ram_option_count: usize_db,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Machine {
	pub name_strindex: usize_db,
	pub source_file_strindex: usize_db,
	pub clone_of_machine_index: usize_db,
	pub rom_of_machine_index: usize_db,
	pub description_strindex: usize_db,
	pub year_strindex: usize_db,
	pub manufacturer_strindex: usize_db,
	pub chips_start: usize_db,
	pub chips_end: usize_db,
	pub devices_start: usize_db,
	pub devices_end: usize_db,
	pub slots_start: usize_db,
	pub slots_end: usize_db,
	pub slot_options_start: usize_db,
	pub slot_options_end: usize_db,
	pub machine_software_lists_start: usize_db,
	pub machine_software_lists_end: usize_db,
	pub ram_options_start: usize_db,
	pub ram_options_end: usize_db,
	pub runnable: bool,
}

impl Fixup for Machine {
	fn identify_machine_indexes(&mut self) -> impl IntoIterator<Item = &mut usize_db> {
		[&mut self.clone_of_machine_index, &mut self.rom_of_machine_index]
	}
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Chip {
	pub clock: U64,
	pub tag_strindex: usize_db,
	pub name_strindex: usize_db,
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
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Device {
	pub type_strindex: usize_db,
	pub tag_strindex: usize_db,
	pub mandatory: bool,
	pub interfaces_strindex: usize_db,
	pub extensions_strindex: usize_db,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct Slot {
	pub name_strindex: usize_db,
	pub options_start: usize_db,
	pub options_end: usize_db,
	pub default_option_index: usize_db,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SlotOption {
	pub name_strindex: usize_db,
	pub devname_strindex: usize_db,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct MachineSoftwareList {
	pub tag_strindex: usize_db,
	pub software_list_index: usize_db,
	pub status: SoftwareListStatus,
	pub filter_strindex: usize_db,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct RamOption {
	pub size: U64,
	pub is_default: bool,
}

impl Fixup for MachineSoftwareList {
	fn identify_software_list_indexes(&mut self) -> impl IntoIterator<Item = &mut usize_db> {
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
	pub name_strindex: usize_db,
	pub software_list_original_machines_start: usize_db,
	pub software_list_compatible_machines_start: usize_db,
	pub software_list_compatible_machines_end: usize_db,
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
