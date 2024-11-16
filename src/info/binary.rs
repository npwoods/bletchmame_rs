use binary_serde::BinarySerde;
use serde::Deserialize;
use strum::EnumString;

pub trait Fixup {
	fn identify_machine_indexes(&mut self) -> impl IntoIterator<Item = &mut u32> {
		[]
	}
	fn identify_software_list_indexes(&mut self) -> impl IntoIterator<Item = &mut u32> {
		[]
	}
}

#[derive(Clone, Copy, Debug, Default, BinarySerde)]
pub struct Header {
	pub magic: [u8; 8],
	pub sizes_hash: u64,
	pub build_strindex: u32,
	pub machine_count: u32,
	pub chips_count: u32,
	pub device_count: u32,
	pub software_list_count: u32,
	pub software_list_machine_count: u32,
	pub machine_software_lists_count: u32,
}

#[derive(Clone, Copy, Debug, Default, BinarySerde)]
pub struct Machine {
	pub name_strindex: u32,
	pub source_file_strindex: u32,
	pub clone_of_machine_index: u32,
	pub rom_of_machine_index: u32,
	pub description_strindex: u32,
	pub year_strindex: u32,
	pub manufacturer_strindex: u32,
	pub chips_start: u32,
	pub chips_end: u32,
	pub devices_start: u32,
	pub devices_end: u32,
	pub machine_software_lists_start: u32,
	pub machine_software_lists_end: u32,
	pub runnable: bool,
}

impl Fixup for Machine {
	fn identify_machine_indexes(&mut self) -> impl IntoIterator<Item = &mut u32> {
		[&mut self.clone_of_machine_index, &mut self.rom_of_machine_index]
	}
}

#[derive(Clone, Copy, Debug, BinarySerde)]
pub struct Chip {
	pub clock: u64,
	pub tag_strindex: u32,
	pub name_strindex: u32,
	pub chip_type: ChipType,
}

#[derive(Clone, Copy, Debug, Deserialize, BinarySerde, EnumString, PartialEq, Eq)]
#[repr(u8)]
pub enum ChipType {
	#[strum(serialize = "cpu")]
	Cpu,
	#[strum(serialize = "audio")]
	Audio,
}

#[derive(Clone, Copy, Debug, BinarySerde)]
pub struct Device {
	pub type_strindex: u32,
	pub tag_strindex: u32,
	pub mandatory: bool,
	pub interface_strindex: u32,
	pub extensions_strindex: u32,
}

#[derive(Clone, Copy, Debug, BinarySerde)]
pub struct MachineSoftwareList {
	pub tag_strindex: u32,
	pub software_list_index: u32,
	pub status: SoftwareListStatus,
	pub filter_strindex: u32,
}

impl Fixup for MachineSoftwareList {
	fn identify_software_list_indexes(&mut self) -> impl IntoIterator<Item = &mut u32> {
		[&mut self.software_list_index]
	}
}

#[derive(Clone, Copy, Debug, Deserialize, BinarySerde, EnumString, PartialEq, Eq)]
#[repr(u8)]
pub enum SoftwareListStatus {
	#[strum(serialize = "original")]
	Original,
	#[strum(serialize = "compatible")]
	Compatible,
}

#[derive(Clone, Copy, Debug, BinarySerde)]
pub struct SoftwareList {
	pub name_strindex: u32,
	pub software_list_original_machines_start: u32,
	pub software_list_compatible_machines_start: u32,
	pub software_list_compatible_machines_end: u32,
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
