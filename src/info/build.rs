use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::BufRead;
use std::ops::ControlFlow;
use std::ops::Index;
use std::ops::Range;
use std::str::FromStr;

use anyhow::Error;
use anyhow::Result;
use easy_ext::ext;
use itertools::Itertools;
use more_asserts::assert_le;
use more_asserts::assert_lt;
use primal::is_prime;
use tracing::debug;
use zerocopy::Immutable;
use zerocopy::IntoBytes;
use zerocopy::little_endian::U64;

use crate::assethash::AssetHash;
use crate::info::ChipType;
use crate::info::MAGIC_HDR;
use crate::info::SERIAL;
use crate::info::SoftwareListStatus;
use crate::info::UsizeDb;
use crate::info::binary;
use crate::info::binary::ASSET_FLAG_BADDUMP;
use crate::info::binary::ASSET_FLAG_HAS_CRC;
use crate::info::binary::ASSET_FLAG_HAS_SHA1;
use crate::info::binary::ASSET_FLAG_NODUMP;
use crate::info::binary::ASSET_FLAG_OPTIONAL;
use crate::info::binary::ASSET_FLAG_WRITABLE;
use crate::info::binary::Fixup;
use crate::info::entities::AssetStatus;
use crate::info::strings::StringTableBuilder;
use crate::info::strip_tag_prefix;
use crate::parse::normalize_tag;
use crate::parse::parse_mame_bool;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	Mame,
	Machine,
	MachineDescription,
	MachineYear,
	MachineManufacturer,
	MachineConfiguration,
	MachineConfigurationSetting,
	MachineDevice,
	MachineSlot,
	MachineRamOption,
}

const TEXT_CAPTURE_PHASES: &[Phase] = &[
	Phase::MachineDescription,
	Phase::MachineYear,
	Phase::MachineManufacturer,
	Phase::MachineRamOption,
];

struct State {
	phase_stack: Vec<Phase>,
	machines: TableBuilder<binary::Machine>,
	roms: TableBuilder<binary::Rom>,
	disks: TableBuilder<binary::Disk>,
	samples: TableBuilder<binary::Sample>,
	biossets: TableBuilder<binary::BiosSet>,
	chips: TableBuilder<binary::Chip>,
	configs: TableBuilder<binary::Configuration>,
	config_settings: TableBuilder<binary::ConfigurationSetting>,
	config_setting_conditions: TableBuilder<binary::ConfigurationSettingCondition>,
	devices: TableBuilder<binary::Device>,
	device_refs: TableBuilder<binary::DeviceRef>,
	slots: TableBuilder<binary::Slot>,
	slot_options: TableBuilder<binary::SlotOption>,
	machine_software_lists: TableBuilder<binary::MachineSoftwareList>,
	strings: StringTableBuilder,
	software_lists: BTreeMap<String, SoftwareListBuild>,
	ram_options: TableBuilder<binary::RamOption>,
	build_strindex: UsizeDb,
	phase_specific: Option<PhaseSpecificState>,
}

enum PhaseSpecificState {
	Extensions(String),
	RamOption(bool),
}

#[derive(Debug, Default)]
struct SoftwareListBuild {
	pub originals: Vec<UsizeDb>,
	pub compatibles: Vec<UsizeDb>,
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Missing mandatory attribute {0} when parsing InfoDB")]
	MissingMandatoryAttribute(&'static str),
	#[error("Bad machine reference in MAME -listxml output: {0}")]
	BadMachineReference(String),
	#[error("Too many records")]
	TooManyRecords,
}

// capacity defaults based on MAME 0.280
//          48893 machines
//         360116 roms
//           1240 disks
//            472 samples
//           4221 BIOS sets
//          24594 chips
//          78106 configurations
//          47581 configuration settings
//            280 configuration setting configurations
//           2890 devices
//          99687 device refs
//           7183 slots
//           3113 slot options
//           7038 links
//           1128 RAM options
//        4997834 string bytes
const CAPACITY_MACHINES: usize = 55000;
const CAPACITY_ROMS: usize = 400000;
const CAPACITY_DISKS: usize = 1600;
const CAPACITY_SAMPLES: usize = 1000;
const CAPACITY_BIOSSETS: usize = 6000;
const CAPACITY_CHIPS: usize = 30000;
const CAPACITY_CONFIGS: usize = 90000;
const CAPACITY_CONFIG_SETTINGS: usize = 52000;
const CAPACITY_CONFIG_SETTING_CONDITIONS: usize = 400;
const CAPACITY_DEVICES: usize = 3500;
const CAPACITY_DEVICE_REFS: usize = 120000;
const CAPACITY_SLOTS: usize = 8500;
const CAPACITY_SLOT_OPTIONS: usize = 3500;
const CAPACITY_MACHINE_SOFTWARE_LISTS: usize = 7500;
const CAPACITY_RAM_OPTIONS: usize = 1500;
const CAPACITY_STRING_TABLE: usize = 5500000;

impl State {
	pub fn new() -> Self {
		// prepare a string table, allocating capacity as above
		let mut strings = StringTableBuilder::new(CAPACITY_STRING_TABLE);

		// placeholder build string, which will be overridden later on
		let build_strindex = strings.lookup("");

		// reserve space based the same MAME version as above
		Self {
			phase_stack: Vec::with_capacity(32),
			machines: TableBuilder::with_capacity(CAPACITY_MACHINES),
			roms: TableBuilder::with_capacity(CAPACITY_ROMS),
			disks: TableBuilder::with_capacity(CAPACITY_DISKS),
			samples: TableBuilder::with_capacity(CAPACITY_SAMPLES),
			biossets: TableBuilder::with_capacity(CAPACITY_BIOSSETS),
			chips: TableBuilder::with_capacity(CAPACITY_CHIPS),
			configs: TableBuilder::with_capacity(CAPACITY_CONFIGS),
			config_settings: TableBuilder::with_capacity(CAPACITY_CONFIG_SETTINGS),
			config_setting_conditions: TableBuilder::with_capacity(CAPACITY_CONFIG_SETTING_CONDITIONS),
			devices: TableBuilder::with_capacity(CAPACITY_DEVICES),
			device_refs: TableBuilder::with_capacity(CAPACITY_DEVICE_REFS),
			slots: TableBuilder::with_capacity(CAPACITY_SLOTS),
			slot_options: TableBuilder::with_capacity(CAPACITY_SLOT_OPTIONS),
			machine_software_lists: TableBuilder::with_capacity(CAPACITY_MACHINE_SOFTWARE_LISTS),
			ram_options: TableBuilder::with_capacity(CAPACITY_RAM_OPTIONS),
			software_lists: BTreeMap::new(),
			strings,
			build_strindex,
			phase_specific: None,
		}
	}

	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> Result<Option<Phase>> {
		debug!(self=?self, evt=?evt, "handle_start()");

		let phase = self.phase_stack.last().unwrap_or(&Phase::Root);
		let evt_name = evt.name();
		let new_phase = match (phase, evt_name.as_ref()) {
			(Phase::Root, b"mame") => {
				let [build] = evt.find_attributes([b"build"])?;
				self.build_strindex = self.strings.lookup(&build.unwrap_or_default());
				Some(Phase::Mame)
			}
			(Phase::Mame, b"machine") => {
				let [name, source_file, clone_of, rom_of, runnable] =
					evt.find_attributes([b"name", b"sourcefile", b"cloneof", b"romof", b"runnable"])?;

				debug!(name =?name, source_file=?source_file, runnable=?runnable,
					"handle_start()"
				);

				let name = name.mandatory("name")?;
				let name_strindex = self.strings.lookup(&name);
				let source_file_strindex = self.strings.lookup(&source_file.unwrap_or_default());
				let clone_of_machine_index = self.strings.lookup(&clone_of.unwrap_or_default());
				let rom_of_machine_index = self.strings.lookup(&rom_of.unwrap_or_default());
				let runnable = runnable.map(parse_mame_bool).transpose()?.unwrap_or(true);

				let machine = binary::Machine {
					name_strindex,
					source_file_strindex,
					clone_of_machine_index,
					rom_of_machine_index,
					roms_start: self.roms.len_db(),
					roms_end: self.roms.len_db(),
					disks_start: self.disks.len_db(),
					disks_end: self.disks.len_db(),
					samples_start: self.samples.len_db(),
					samples_end: self.samples.len_db(),
					biossets_start: self.biossets.len_db(),
					biossets_end: self.biossets.len_db(),
					default_biosset_index: !UsizeDb::default(),
					chips_start: self.chips.len_db(),
					chips_end: self.chips.len_db(),
					configs_start: self.configs.len_db(),
					configs_end: self.configs.len_db(),
					devices_start: self.devices.len_db(),
					devices_end: self.devices.len_db(),
					device_refs_start: self.device_refs.len_db(),
					device_refs_end: self.device_refs.len_db(),
					slots_start: self.slots.len_db(),
					slots_end: self.slots.len_db(),
					machine_software_lists_start: self.machine_software_lists.len_db(),
					machine_software_lists_end: self.machine_software_lists.len_db(),
					ram_options_start: self.ram_options.len_db(),
					ram_options_end: self.ram_options.len_db(),
					runnable,
					..Default::default()
				};
				self.machines.push_db(machine)?;
				Some(Phase::Machine)
			}
			(Phase::Machine, b"description") => Some(Phase::MachineDescription),
			(Phase::Machine, b"year") => Some(Phase::MachineYear),
			(Phase::Machine, b"manufacturer") => Some(Phase::MachineManufacturer),
			(Phase::Machine, b"rom") => {
				let [name, bios, size, crc, sha1, region, offset, status, optional] = evt.find_attributes([
					b"name",
					b"bios",
					b"size",
					b"crc",
					b"sha1",
					b"region",
					b"offset",
					b"status",
					b"optional",
				])?;
				let name = name.mandatory("name")?;
				let size = size.as_deref().map(str::parse::<u64>).transpose()?;
				let size = size.unwrap_or(!0).into();
				let asset_hash = AssetHash::from_hex_strings(crc.as_deref(), sha1.as_deref())?;
				let crc = asset_hash.crc.unwrap_or_default().into();
				let sha1 = asset_hash.sha1.unwrap_or_default();
				let region = region.mandatory("region")?;
				let offset = offset.as_deref().map(|src| u64::from_str_radix(src, 16)).transpose()?;
				let offset = offset.unwrap_or(!0).into();
				let name_strindex = self.strings.lookup(&name);
				let bios_strindex = bios
					.map(|bios| self.strings.lookup(&bios))
					.unwrap_or(!UsizeDb::default());
				let region_strindex = self.strings.lookup(&region);
				let optional = optional.map(parse_mame_bool).transpose()?.unwrap_or_default();
				let status = status.as_deref().map(AssetStatus::from_str).transpose()?;
				let flags = make_asset_flags(&asset_hash, optional, false, status);
				let rom = binary::Rom {
					name_strindex,
					bios_strindex,
					size,
					crc,
					sha1,
					region_strindex,
					offset,
					flags,
				};
				self.roms.push_db(rom)?;
				self.machines.modify_last(|machine| {
					machine.roms_end += 1;
				});
				None
			}
			(Phase::Machine, b"disk") => {
				let [name, merge, sha1, region, index, writable, status, optional] = evt.find_attributes([
					b"name",
					b"merge",
					b"sha1",
					b"region",
					b"index",
					b"writable",
					b"status",
					b"optional",
				])?;
				let name = name.mandatory("name")?;
				let asset_hash = AssetHash::from_hex_strings(None, sha1.as_deref())?;
				let sha1 = asset_hash.sha1.unwrap_or_default();
				let region = region.mandatory("region")?;
				let index = index.as_deref().map(|src| u64::from_str_radix(src, 16)).transpose()?;
				let index = index.unwrap_or(!0).into();
				let writable = writable.map(parse_mame_bool).transpose()?.unwrap_or_default();
				let name_strindex = self.strings.lookup(&name);
				let merge_strindex = merge
					.as_ref()
					.map(|x| self.strings.lookup(x))
					.unwrap_or(!UsizeDb::default());
				let region_strindex = self.strings.lookup(&region);
				let optional = optional.map(parse_mame_bool).transpose()?.unwrap_or_default();
				let status = status.as_deref().map(AssetStatus::from_str).transpose()?;
				let flags = make_asset_flags(&asset_hash, optional, writable, status);
				let disk = binary::Disk {
					name_strindex,
					merge_strindex,
					sha1,
					region_strindex,
					index,
					flags,
				};
				self.disks.push_db(disk)?;
				self.machines.modify_last(|machine| {
					machine.disks_end += 1;
				});
				None
			}
			(Phase::Machine, b"sample") => {
				let [name] = evt.find_attributes([b"name"])?;
				let name = name.mandatory("name")?;
				let name_strindex = self.strings.lookup(&name);
				let sample = binary::Sample { name_strindex };
				self.samples.push_db(sample)?;
				self.machines.modify_last(|machine| {
					machine.samples_end += 1;
				});
				None
			}
			(Phase::Machine, b"biosset") => {
				let [name, description, is_default] = evt.find_attributes([b"name", b"description", b"default"])?;
				let name_strindex = self.strings.lookup(&name.mandatory("name")?);
				let description_strindex = self.strings.lookup(&description.mandatory("description")?);
				let is_default = is_default.map(parse_mame_bool).transpose()?.unwrap_or(false);

				if is_default {
					self.machines.modify_last(|machine| {
						machine.default_biosset_index = machine.biossets_end - machine.biossets_start;
					});
				}

				let biosset = binary::BiosSet {
					name_strindex,
					description_strindex,
				};
				self.biossets.push_db(biosset)?;
				self.machines.modify_last(|machine| {
					machine.biossets_end += 1;
				});
				None
			}
			(Phase::Machine, b"chip") => {
				let [chip_type, tag, name, clock] = evt.find_attributes([b"type", b"tag", b"name", b"clock"])?;
				let Ok(chip_type) = (*chip_type.mandatory("type")?).parse::<ChipType>() else {
					// presumably an unknown chip type; ignore
					return Ok(None);
				};
				let tag_strindex = self.strings.lookup(&tag.unwrap_or_default());
				let name_strindex = self.strings.lookup(&name.unwrap_or_default());
				let clock = clock.as_ref().and_then(|x| x.parse().ok()).unwrap_or(0).into();
				let chip = binary::Chip {
					chip_type,
					tag_strindex,
					name_strindex,
					clock,
				};
				self.chips.push_db(chip)?;
				self.machines.modify_last(|machine| {
					machine.chips_end += 1;
				});
				None
			}
			(Phase::Machine, b"configuration" | b"dipswitch") => {
				let [name, tag, mask] = evt.find_attributes([b"name", b"tag", b"mask"])?;
				let name = name.mandatory("name")?;
				let tag = normalize_tag(tag.mandatory("tag")?);
				let mask = (*mask.mandatory("mask")?).parse::<u32>()?.into();
				let name_strindex = self.strings.lookup(&name);
				let tag_strindex = self.strings.lookup(&tag);
				let config = binary::Configuration {
					name_strindex,
					tag_strindex,
					mask,
					settings_start: self.config_settings.len_db(),
					settings_end: self.config_settings.len_db(),
					default_setting_index: !UsizeDb::default(),
				};
				self.configs.push_db(config)?;
				self.machines.modify_last(|machine| {
					machine.configs_end += 1;
				});
				Some(Phase::MachineConfiguration)
			}
			(Phase::MachineConfiguration, b"confsetting" | b"dipvalue") => {
				let [name, value, is_default] = evt.find_attributes([b"name", b"value", b"default"])?;
				let name_strindex = self.strings.lookup(&name.mandatory("name")?);
				let value = value.mandatory("value")?.parse::<u32>()?.into();
				let is_default = is_default.map(parse_mame_bool).transpose()?.unwrap_or(false);

				if is_default {
					self.configs.modify_last(|config| {
						config.default_setting_index = config.settings_end - config.settings_start;
					});
				}

				let config_setting = binary::ConfigurationSetting {
					name_strindex,
					value,
					conditions_start: self.config_setting_conditions.len_db(),
					conditions_end: self.config_setting_conditions.len_db(),
				};
				self.config_settings.push_db(config_setting)?;
				self.configs.modify_last(|machine| {
					machine.settings_end += 1;
				});
				Some(Phase::MachineConfigurationSetting)
			}
			(Phase::MachineConfigurationSetting, b"condition") => {
				let [tag, relation, mask, value] = evt.find_attributes([b"tag", b"relation", b"mask", b"value"])?;
				let tag = normalize_tag(tag.mandatory("tag")?);
				let tag_strindex = self.strings.lookup(&tag);
				let Ok(condition_relation) = relation.mandatory("relation")?.parse::<binary::ConditionRelation>()
				else {
					// presumably an unknown condition relation; ignore
					return Ok(None);
				};
				let mask = (*mask.mandatory("mask")?).parse::<u32>()?.into();
				let value = (*value.mandatory("value")?).parse::<u32>()?.into();
				let condition = binary::ConfigurationSettingCondition {
					tag_strindex,
					condition_relation,
					mask,
					value,
				};
				self.config_setting_conditions.push_db(condition)?;
				self.config_settings.modify_last(|machine| {
					machine.conditions_end += 1;
				});
				None
			}
			(Phase::Machine, b"device") => {
				let [device_type, tag, mandatory, interface] =
					evt.find_attributes([b"type", b"tag", b"mandatory", b"interface"])?;
				let tag = tag.mandatory("tag")?;
				let tag = normalize_tag(tag);
				let type_strindex = self.strings.lookup(&device_type.unwrap_or_default());
				let tag_strindex = self.strings.lookup(&tag);
				let mandatory = mandatory.map(parse_mame_bool).transpose()?.unwrap_or(false);
				let interfaces = interface.unwrap_or_default().split(',').sorted().join("\0");
				let interfaces_strindex = self.strings.lookup(&interfaces);
				let device = binary::Device {
					type_strindex,
					tag_strindex,
					mandatory,
					interfaces_strindex,
					extensions_strindex: UsizeDb::default(),
				};
				self.devices.push_db(device)?;
				self.machines.modify_last(|machine| {
					machine.devices_end += 1;
				});
				self.phase_specific = Some(PhaseSpecificState::Extensions(String::with_capacity(1024)));
				Some(Phase::MachineDevice)
			}
			(Phase::Machine, b"device_ref") => {
				let [name, tag] = evt.find_attributes([b"name", b"tag"])?;
				let name = name.mandatory("name")?;
				let name_strindex = self.strings.lookup(&name);
				let tag = tag.map(normalize_tag);
				let tag_strindex = tag.map(|tag| self.strings.lookup(&tag)).unwrap_or(!UsizeDb::default());
				let device_ref = binary::DeviceRef {
					machine_index: name_strindex,
					tag_strindex,
				};
				self.device_refs.push_db(device_ref)?;
				self.machines.modify_last(|machine| {
					machine.device_refs_end += 1;
				});
				None
			}
			(Phase::Machine, b"slot") => {
				let [name] = evt.find_attributes([b"name"])?;
				let name = name.mandatory("slot")?;
				let name = normalize_tag(name);
				let name_strindex = self.strings.lookup(&name);
				let slot_options_pos = self.slot_options.len_db();
				let slot = binary::Slot {
					name_strindex,
					options_start: slot_options_pos,
					options_end: slot_options_pos,
					default_option_index: !UsizeDb::default(),
				};
				self.slots.push_db(slot)?;
				self.machines.modify_last(|machine| {
					machine.slots_end += 1;
				});
				Some(Phase::MachineSlot)
			}
			(Phase::Machine, b"softwarelist") => {
				let [tag, name, status, filter] = evt.find_attributes([b"tag", b"name", b"status", b"filter"])?;
				let status = status.mandatory("status")?;
				let Ok(status) = (*status).parse::<SoftwareListStatus>() else {
					// presumably an unknown software list status; ignore
					return Ok(None);
				};
				let name = name.mandatory("name")?;
				let tag_strindex = self.strings.lookup(&tag.unwrap_or_default());
				let name_strindex = self.strings.lookup(&name);
				let filter_strindex = self.strings.lookup(&filter.unwrap_or_default());
				let machine_software_list = binary::MachineSoftwareList {
					tag_strindex,
					software_list_index: name_strindex,
					status,
					filter_strindex,
				};
				self.machine_software_lists.push_db(machine_software_list)?;
				let machine_name_strindex = self.machines.modify_last(|machine| {
					machine.machine_software_lists_end += 1;
					machine.name_strindex
				});

				// add this machine to the global software list
				let software_list = self.software_lists.entry(name.to_string()).or_default();
				let list = match status {
					SoftwareListStatus::Original => &mut software_list.originals,
					SoftwareListStatus::Compatible => &mut software_list.compatibles,
				};
				list.push(machine_name_strindex);
				None
			}
			(Phase::Machine, b"ramoption") => {
				let [is_default] = evt.find_attributes([b"default"])?;
				let is_default = is_default.map(parse_mame_bool).transpose()?.unwrap_or_default();
				self.phase_specific = Some(PhaseSpecificState::RamOption(is_default));
				Some(Phase::MachineRamOption)
			}
			(Phase::MachineDevice, b"extension") => {
				let [name] = evt.find_attributes([b"name"])?;
				if let Some(name) = name {
					let PhaseSpecificState::Extensions(extensions) = self.phase_specific.as_mut().unwrap() else {
						unreachable!();
					};
					if !extensions.is_empty() {
						extensions.push('\0');
					}
					extensions.push_str(name.as_ref());
				}
				None
			}
			(Phase::MachineSlot, b"slotoption") => {
				let [name, devname, is_default] = evt.find_attributes([b"name", b"devname", b"default"])?;
				let name = name.mandatory("name")?;
				let devname = devname.mandatory("devname")?;
				let name_strindex = self.strings.lookup(&name);
				let devname_strindex = self.strings.lookup(&devname);
				let is_default = is_default.map(parse_mame_bool).transpose()?.unwrap_or_default();
				if is_default {
					self.slots.modify_last(|slot| {
						slot.default_option_index = slot.options_end - slot.options_start;
					});
				}
				let slot_option = binary::SlotOption {
					name_strindex,
					devname_strindex,
				};
				self.slots.modify_last(|slot| {
					slot.options_end += 1;
				});
				self.slot_options.push_db(slot_option)?;
				None
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(
		&mut self,
		callback: &mut impl FnMut(&str) -> ControlFlow<()>,
		text: Option<String>,
	) -> Result<Option<()>> {
		debug!(self=?self, "handle_end()");

		match self.phase_stack.last().unwrap_or(&Phase::Root) {
			Phase::Machine => {
				self.machines.modify_last(|machine| {
					// MAME will emit "phantom" slots pertaining to the default configuration; while this is considered to be a
					// feature and not a bug (see https://github.com/mamedev/mame/pull/14639#issuecomment-3620541605), this is bad
					// for us; therefore we need to prune them
					let slot_tags = self.slots[usize::from(machine.slots_start)..usize::from(machine.slots_end)]
						.iter()
						.map(|s| self.strings.index(s.name_strindex))
						.collect::<Vec<_>>();
					filter_tail(machine.slots_start, &mut machine.slots_end, &mut self.slots, |s| {
						let slot_name = self.strings.index(s.name_strindex);
						!slot_tags
							.iter()
							.any(|x| strip_tag_prefix(slot_name, x).is_some_and(|t| !t.is_empty()))
					});
					filter_tail(
						machine.device_refs_start,
						&mut machine.device_refs_end,
						&mut self.device_refs,
						|dr| {
							dr.tag_strindex == !UsizeDb::default() || {
								let tag = self.strings.index(dr.tag_strindex);
								!slot_tags
									.iter()
									.any(|x| strip_tag_prefix(tag, x).is_some_and(|t| !t.is_empty()))
							}
						},
					);

					// lots of machines look similar, and their collections are similar - attempt to reuse
					// existing collections
					try_reuse_existing_collection(&mut machine.roms_start, &mut machine.roms_end, &mut self.roms);
					try_reuse_existing_collection(&mut machine.disks_start, &mut machine.disks_end, &mut self.disks);
					try_reuse_existing_collection(
						&mut machine.samples_start,
						&mut machine.samples_end,
						&mut self.samples,
					);
					try_reuse_existing_collection(
						&mut machine.biossets_start,
						&mut machine.biossets_end,
						&mut self.biossets,
					);
					try_reuse_existing_collection(&mut machine.chips_start, &mut machine.chips_end, &mut self.chips);
					try_reuse_existing_collection(
						&mut machine.configs_start,
						&mut machine.configs_end,
						&mut self.configs,
					);
					try_reuse_existing_collection(
						&mut machine.devices_start,
						&mut machine.devices_end,
						&mut self.devices,
					);
					try_reuse_existing_collection(
						&mut machine.device_refs_start,
						&mut machine.device_refs_end,
						&mut self.device_refs,
					);
					try_reuse_existing_collection(&mut machine.slots_start, &mut machine.slots_end, &mut self.slots);
					try_reuse_existing_collection(
						&mut machine.ram_options_start,
						&mut machine.ram_options_end,
						&mut self.ram_options,
					);
				});
			}
			Phase::MachineConfiguration => {
				self.configs.modify_last(|config| {
					try_reuse_existing_collection(
						&mut config.settings_start,
						&mut config.settings_end,
						&mut self.config_settings,
					);
				});
			}
			Phase::MachineConfigurationSetting => {
				self.config_settings.modify_last(|setting| {
					try_reuse_existing_collection(
						&mut setting.conditions_start,
						&mut setting.conditions_end,
						&mut self.config_setting_conditions,
					);
				});
			}
			Phase::MachineDescription => {
				let description = text.unwrap();
				if !description.is_empty() && callback(&description).is_break() {
					return Ok(None);
				}
				let description_strindex = self.strings.lookup(&description);
				self.machines.modify_last(|machine| {
					machine.description_strindex = description_strindex;
				});
			}
			Phase::MachineYear => {
				let year_strindex = self.strings.lookup(&text.unwrap());
				self.machines.modify_last(|machine| {
					machine.year_strindex = year_strindex;
				});
			}
			Phase::MachineManufacturer => {
				let manufacturer_strindex = self.strings.lookup(&text.unwrap());
				self.machines.modify_last(|machine| {
					machine.manufacturer_strindex = manufacturer_strindex;
				});
			}
			Phase::MachineDevice => {
				let PhaseSpecificState::Extensions(extensions) = self.phase_specific.take().unwrap() else {
					unreachable!();
				};
				let extensions = extensions.split('\0').sorted().join("\0");
				let extensions_strindex = self.strings.lookup(&extensions);
				self.devices.modify_last(|device| {
					device.extensions_strindex = extensions_strindex;
				});
			}
			Phase::MachineSlot => {
				self.slots.modify_last(|slot| {
					try_reuse_existing_collection(
						&mut slot.options_start,
						&mut slot.options_end,
						&mut self.slot_options,
					);
				});
			}
			Phase::MachineRamOption => {
				let PhaseSpecificState::RamOption(is_default) = self.phase_specific.take().unwrap() else {
					unreachable!();
				};
				if let Ok(size) = text.unwrap().parse::<u64>() {
					let size = size.into();
					let ram_option = binary::RamOption { size, is_default };
					self.ram_options.push_db(ram_option)?;
					self.machines.modify_last(|machine| {
						machine.ram_options_end += 1;
					});
				}
			}
			_ => {}
		};
		Ok(Some(()))
	}

	pub fn into_data(self) -> Result<Box<[u8]>> {
		// we need to do processing on machines, namely to  canonicalize name_strindex, so we
		// don't have both inline and indexed small sting references, and sort it
		let machine_count = self.machines.len_db();
		let mut machines = self
			.machines
			.into_vec()
			.into_iter()
			.map(|machine| {
				let old_strindex = machine.name_strindex;
				let new_strindex = self
					.strings
					.lookup_immut(self.strings.index(old_strindex))
					.unwrap_or(old_strindex);

				binary::Machine {
					name_strindex: new_strindex,
					..machine
				}
			})
			.sorted_by_key(|m| self.strings.index(m.name_strindex))
			.collect::<Vec<_>>();

		// build a "machine.name_strindex" ==> "machine_index" map in preparations for fixups
		let machines_indexmap = machines
			.iter()
			.enumerate()
			.map(|(index, obj)| (obj.name_strindex, index.try_into().unwrap()))
			.collect::<HashMap<_, _>>();
		let machines_indexmap = |strindex| {
			let result = machines_indexmap
				.get(&strindex)
				.or_else(|| {
					let new_strindex = self.strings.lookup_immut(self.strings.index(strindex));
					new_strindex.and_then(|x| machines_indexmap.get(&x))
				})
				.copied();

			// sanity check and return
			assert!(result.is_none_or(|x| x < machine_count), "Invalid machine");
			result
		};

		// software lists require special processing
		let mut software_list_machine_indexes = Vec::<UsizeDb>::with_capacity(CAPACITY_MACHINE_SOFTWARE_LISTS);
		let mut software_list_indexmap = HashMap::new();
		let software_lists = self
			.software_lists
			.into_iter()
			.enumerate()
			.map(|(index, (software_list_name, data))| {
				let originals = data
					.originals
					.into_iter()
					.filter_map(machines_indexmap)
					.sorted()
					.collect::<Vec<_>>();
				let compatibles = data
					.compatibles
					.into_iter()
					.filter_map(machines_indexmap)
					.sorted()
					.collect::<Vec<_>>();
				let index = UsizeDb::try_from(index).unwrap();
				let name_strindex = self.strings.lookup_immut(&software_list_name).unwrap();
				let software_list_original_machines_start = software_list_machine_indexes.len();
				let software_list_compatible_machines_start = software_list_original_machines_start + originals.len();
				let software_list_compatible_machines_end = software_list_compatible_machines_start + compatibles.len();
				let software_list_original_machines_start = software_list_original_machines_start.try_into().unwrap();
				let software_list_compatible_machines_start =
					software_list_compatible_machines_start.try_into().unwrap();
				let software_list_compatible_machines_end = software_list_compatible_machines_end.try_into().unwrap();
				let entry = binary::SoftwareList {
					name_strindex,
					software_list_original_machines_start,
					software_list_compatible_machines_start,
					software_list_compatible_machines_end,
				};
				assert!(originals.iter().all(|&x| x < machine_count));
				assert!(compatibles.iter().all(|&x| x < machine_count));

				software_list_machine_indexes.extend(originals);
				software_list_machine_indexes.extend(compatibles);
				software_list_indexmap.insert(name_strindex, index);
				entry
			})
			.collect::<Vec<_>>();

		// resolves `software_list_index` entries that are actually name `strindex`
		// values; part of the obnoxiousness is caused by how these can be short names
		let software_list_indexmap = |software_list_index| {
			let index = *software_list_indexmap
				.get(&software_list_index)
				.or_else(|| {
					let software_list_index = self
						.strings
						.lookup_immut(self.strings.index(software_list_index))
						.unwrap();
					software_list_indexmap.get(&software_list_index)
				})
				.unwrap();
			assert_lt!(index, software_lists.len_db());
			index
		};

		// and run the fixups
		let mut device_refs = self.device_refs.into_vec();
		let mut machine_software_lists = self.machine_software_lists.into_vec();
		fixup(&mut machines, &self.strings, machines_indexmap, software_list_indexmap)?;
		fixup(
			&mut device_refs,
			&self.strings,
			machines_indexmap,
			software_list_indexmap,
		)?;
		fixup(
			&mut machine_software_lists,
			&self.strings,
			machines_indexmap,
			software_list_indexmap,
		)?;

		// assemble the header
		let header = binary::Header {
			magic: *MAGIC_HDR,
			serial: SERIAL.into(),
			sizes_hash: calculate_sizes_hash(),
			build_strindex: self.build_strindex,
			machine_count: machines.len_db(),
			rom_count: self.roms.len_db(),
			disk_count: self.disks.len_db(),
			sample_count: self.samples.len_db(),
			biosset_count: self.biossets.len_db(),
			chips_count: self.chips.len_db(),
			config_count: self.configs.len_db(),
			config_setting_count: self.config_settings.len_db(),
			config_setting_condition_count: self.config_setting_conditions.len_db(),
			device_count: self.devices.len_db(),
			device_ref_count: device_refs.len_db(),
			slot_count: self.slots.len_db(),
			slot_option_count: self.slot_options.len_db(),
			software_list_count: software_lists.len_db(),
			software_list_machine_count: software_list_machine_indexes.len_db(),
			machine_software_lists_count: machine_software_lists.len_db(),
			ram_option_count: self.ram_options.len_db(),
		};

		// get all bytes and return
		let bytes = header
			.as_bytes()
			.iter()
			.chain(machines.iter().flat_map(IntoBytes::as_bytes))
			.chain(self.roms.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(self.disks.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(self.samples.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(self.biossets.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(self.chips.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(self.configs.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(self.config_settings.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(
				self.config_setting_conditions
					.into_vec()
					.iter()
					.flat_map(IntoBytes::as_bytes),
			)
			.chain(self.devices.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(device_refs.iter().flat_map(IntoBytes::as_bytes))
			.chain(self.slots.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(self.slot_options.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.chain(software_lists.iter().flat_map(IntoBytes::as_bytes))
			.chain(software_list_machine_indexes.iter().flat_map(IntoBytes::as_bytes))
			.chain(machine_software_lists.iter().flat_map(IntoBytes::as_bytes))
			.chain(self.ram_options.into_vec().iter().flat_map(IntoBytes::as_bytes))
			.copied()
			.chain(self.strings.into_iter())
			.collect();
		Ok(bytes)
	}
}

impl Debug for State {
	fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
		f.debug_struct("State")
			.field("phase_stack", &self.phase_stack)
			.field("machines.len()", &self.machines.len())
			.field("chips.len()", &self.chips.len())
			.finish_non_exhaustive()
	}
}

fn fixup(
	vec: &mut [impl Fixup + Immutable + IntoBytes],
	strings: &StringTableBuilder,
	machines_indexmap: impl Fn(UsizeDb) -> Option<UsizeDb>,
	software_list_indexmap: impl Fn(UsizeDb) -> UsizeDb,
) -> Result<()> {
	for x in vec.iter_mut() {
		for machine_index in x.identify_machine_indexes() {
			let new_machine_index = if *machine_index != UsizeDb::default() {
				machines_indexmap(*machine_index).ok_or_else(|| {
					let machine_name = strings.index(*machine_index).into();
					ThisError::BadMachineReference(machine_name)
				})?
			} else {
				!UsizeDb::default()
			};
			*machine_index = new_machine_index;
		}
		for machine_index in x.identify_optional_machine_indexes() {
			*machine_index = machines_indexmap(*machine_index).unwrap_or_else(|| !UsizeDb::default());
		}
		for software_list_index in x.identify_software_list_indexes() {
			*software_list_index = software_list_indexmap(*software_list_index);
		}
	}
	Ok(())
}

fn filter_tail<T>(
	start: UsizeDb,
	end: &mut UsizeDb,
	table_builder: &mut TableBuilder<T>,
	predicate: impl Fn(&T) -> bool,
) where
	T: Eq + Hash,
{
	// sanity checks
	assert_le!(start, *end);
	assert_eq!(*end, table_builder.len_db());

	let count = usize::from(*end) - usize::from(start);
	let new_count = table_builder.filter_tail(count, predicate);
	*end = (usize::from(start) + new_count).try_into().unwrap();
}

fn try_reuse_existing_collection(
	start: &mut UsizeDb,
	end: &mut UsizeDb,
	table_builder: &mut TableBuilder<impl Eq + Hash>,
) {
	// sanity checks
	assert_le!(*start, *end);
	assert_eq!(*end, table_builder.len_db());

	let count = usize::from(*end) - usize::from(*start);
	if let Some(pos) = table_builder.try_recycle_tail(count) {
		*start = pos.try_into().unwrap();
		*end = (pos + count).try_into().unwrap();
	}
}

fn listxml_err(reader: &XmlReader<impl BufRead>, e: impl Into<Error>) -> Error {
	let message = format!(
		"Error processing MAME -listxml output at position {}",
		reader.buffer_position()
	);
	e.into().context(message)
}

pub fn data_from_listxml_output(
	reader: impl BufRead,
	mut callback: impl FnMut(&str) -> ControlFlow<()>,
) -> Result<Option<Box<[u8]>>> {
	let mut state = State::new();
	let mut reader = XmlReader::from_reader(reader, true);
	let mut buf = Vec::with_capacity(1024);

	while let Some(evt) = reader.next(&mut buf).map_err(|e| listxml_err(&reader, e))? {
		match evt {
			XmlEvent::Start(evt) => {
				let new_phase = state.handle_start(evt).map_err(|e| listxml_err(&reader, e))?;

				if let Some(new_phase) = new_phase {
					state.phase_stack.push(new_phase);

					if TEXT_CAPTURE_PHASES.contains(&new_phase) {
						reader.start_text_capture();
					}
				} else {
					reader.start_unknown_tag();
				}
			}

			XmlEvent::End(s) => {
				let result = state
					.handle_end(&mut callback, s)
					.map_err(|e| listxml_err(&reader, e))?;
				if result.is_none() {
					// user cancelled out
					return Ok(None);
				}
				state.phase_stack.pop().unwrap();
			}

			XmlEvent::Null => {} // meh
		}
	}

	// sanity check
	assert!(state.phase_stack.is_empty());

	// get all bytes and return
	let data = state.into_data()?;
	Ok(Some(data))
}

pub fn calculate_sizes_hash() -> U64 {
	let multiplicand = 4733; // arbitrary prime number
	[
		size_of::<binary::Header>(),
		size_of::<binary::Machine>(),
		size_of::<binary::Rom>(),
		size_of::<binary::Disk>(),
		size_of::<binary::Sample>(),
		size_of::<binary::BiosSet>(),
		size_of::<binary::Chip>(),
		size_of::<binary::Configuration>(),
		size_of::<binary::ConfigurationSetting>(),
		size_of::<binary::ConfigurationSettingCondition>(),
		size_of::<binary::Device>(),
		size_of::<binary::DeviceRef>(),
		size_of::<binary::Slot>(),
		size_of::<binary::SlotOption>(),
		size_of::<binary::SoftwareList>(),
		size_of::<binary::MachineSoftwareList>(),
		size_of::<binary::RamOption>(),
	]
	.into_iter()
	.fold(0, |value, item| {
		u64::overflowing_mul(value, multiplicand).0 ^ (item as u64)
	})
	.into()
}

struct TableBuilder<T> {
	data: Vec<T>,
	buckets: Box<[Vec<usize>]>,
}

impl<T> TableBuilder<T> {
	pub fn with_capacity(capacity: usize) -> Self {
		let bucket_count = ((capacity / 32 + 100)..)
			.find(|&n| is_prime(n.try_into().unwrap()))
			.unwrap();
		let data = Vec::with_capacity(capacity);
		let buckets = (0..bucket_count).map(|_| Vec::new()).collect::<Box<[_]>>();
		Self { data, buckets }
	}

	pub fn len(&self) -> usize {
		self.data.len()
	}

	pub fn len_db(&self) -> UsizeDb {
		self.data.len_db()
	}

	pub fn into_vec(self) -> Vec<T> {
		self.data
	}
}

impl<T> TableBuilder<T>
where
	T: Hash + Eq,
{
	pub fn push_db(&mut self, value: T) -> Result<()> {
		self.buckets[self.bucket_index(&value)].push(self.data.len());
		self.data.push(value);
		self.data.try_len_db().map(|_| ())
	}

	pub fn pop(&mut self) -> Option<T> {
		let value = self.data.pop()?;
		self.buckets[self.bucket_index(&value)].pop().unwrap();
		Some(value)
	}

	pub fn modify_last<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
		let mut value = self.pop().unwrap();
		let result = f(&mut value);
		self.push_db(value).unwrap();
		result
	}

	pub fn filter_tail(&mut self, tail_len: usize, predicate: impl Fn(&T) -> bool) -> usize {
		let mut tail = Vec::new();
		for _ in 0..tail_len {
			if let Some(x) = self.pop()
				&& predicate(&x)
			{
				tail.push(x);
			}
		}

		let new_tail_len = tail.len();
		for x in tail.into_iter().rev() {
			self.push_db(x).unwrap();
		}

		new_tail_len
	}

	pub fn try_recycle_tail(&mut self, tail_len: usize) -> Option<usize> {
		// separate out the 'tail' and the 'body'; after this they
		// will be treated distinctly
		let tail_pos = self.data.len() - tail_len;
		let tail = &self.data[tail_pos..];
		let body = &self.data[..tail_pos];

		if tail.is_empty() {
			Some(0)
		} else {
			let index = self.buckets[self.bucket_index(&tail[0])]
				.iter()
				.rev()
				.copied()
				.find(|index| body.get(*index..(*index + tail_len)) == Some(tail));

			if index.is_some() {
				for _ in 0..tail_len {
					self.pop();
				}
			}
			index
		}
	}

	fn bucket_index(&self, value: &T) -> usize {
		let mut hasher = DefaultHasher::new();
		value.hash(&mut hasher);
		(hasher.finish() as usize) % self.buckets.len()
	}
}

impl<T> Index<Range<usize>> for TableBuilder<T> {
	type Output = [T];

	fn index(&self, range: Range<usize>) -> &Self::Output {
		&self.data[range]
	}
}

#[ext]
impl<T> Vec<T> {
	pub fn len_db(&self) -> UsizeDb {
		self.try_len_db().unwrap()
	}

	pub fn try_len_db(&self) -> Result<UsizeDb> {
		self.len().try_into().map_err(|_| ThisError::TooManyRecords.into())
	}
}

#[ext]
impl<T> Option<T> {
	pub fn mandatory(self, name: &'static str) -> Result<T> {
		self.ok_or(ThisError::MissingMandatoryAttribute(name).into())
	}
}

fn make_asset_flags(asset_hash: &AssetHash, optional: bool, writable: bool, status: Option<AssetStatus>) -> u8 {
	let status = status.unwrap_or(AssetStatus::Good);
	(asset_hash.crc.is_some() as u8 * ASSET_FLAG_HAS_CRC)
		| (asset_hash.sha1.is_some() as u8 * ASSET_FLAG_HAS_SHA1)
		| (writable as u8 * ASSET_FLAG_WRITABLE)
		| (optional as u8 * ASSET_FLAG_OPTIONAL)
		| ((status == AssetStatus::BadDump) as u8 * ASSET_FLAG_BADDUMP)
		| ((status == AssetStatus::NoDump) as u8 * ASSET_FLAG_NODUMP)
}

#[cfg(test)]
mod test {
	use std::io::BufReader;
	use std::ops::ControlFlow;

	use assert_matches::assert_matches;
	use test_case::test_case;

	use super::super::InfoDb;

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"))]
	#[test_case(1, include_str!("test_data/listxml_coco.xml"))]
	#[test_case(2, include_str!("test_data/listxml_fake.xml"))]
	pub fn data_from_listxml_output(_index: usize, xml: &str) {
		let reader = BufReader::new(xml.as_bytes());
		let data = super::data_from_listxml_output(reader, |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let result = InfoDb::new(data);
		assert_matches!(result, Ok(_));
	}

	#[test_case(0, &[], 0, &[], Some(0))]
	#[test_case(1, &[10, 11, 12, 13, 14, 15], 0, &[10, 11, 12, 13, 14, 15], Some(0))]
	#[test_case(2, &[10, 11, 12, 13, 14, 15], 3, &[10, 11, 12, 13, 14, 15], None)]
	#[test_case(3, &[10, 11, 12, 13, 14, 15, 13, 14, 15], 2, &[10, 11, 12, 13, 14, 15, 13], Some(4))]
	#[test_case(4, &[10, 11, 12, 13, 14, 15, 13, 14, 15], 3, &[10, 11, 12, 13, 14, 15], Some(3))]
	#[test_case(5, &[99, 99, 99, 99, 98], 2, &[99, 99, 99, 99, 98], None)]
	#[test_case(6, &[99, 99, 99, 99, 99], 2, &[99, 99, 99], Some(1))]
	#[test_case(7, &[99, 99, 11, 99, 99], 2, &[99, 99, 11], Some(0))]
	pub fn try_recycle_tail(
		_index: usize,
		initial_table: &[u32],
		tail_len: usize,
		expected_table: &[u32],
		expected_pos: Option<usize>,
	) {
		let mut table_builder = super::TableBuilder::with_capacity(initial_table.len());
		for &value in initial_table {
			table_builder.push_db(value).unwrap();
		}
		let pos = table_builder.try_recycle_tail(tail_len);
		let vec = table_builder.into_vec();
		let actual = (vec.as_slice(), pos);
		assert_eq!((expected_table, expected_pos), actual);
	}
}
