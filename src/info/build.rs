use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::io::BufRead;
use std::marker::PhantomData;

use anyhow::Error;
use anyhow::Result;
use binary_serde::BinarySerde;
use itertools::Itertools;
use num::CheckedAdd;
use tracing::Level;
use tracing::event;

use crate::info::ChipType;
use crate::info::ENDIANNESS;
use crate::info::MAGIC_HDR;
use crate::info::SoftwareListStatus;
use crate::info::binary;
use crate::info::binary::Fixup;
use crate::info::strings::StringTableBuilder;
use crate::parse::normalize_tag;
use crate::parse::parse_mame_bool;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;

const LOG: Level = Level::DEBUG;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	Mame,
	Machine,
	MachineDescription,
	MachineYear,
	MachineManufacturer,
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
	machines: BinBuilder<binary::Machine>,
	chips: BinBuilder<binary::Chip>,
	devices: BinBuilder<binary::Device>,
	slots: BinBuilder<binary::Slot>,
	slot_options: BinBuilder<binary::SlotOption>,
	machine_software_lists: BinBuilder<binary::MachineSoftwareList>,
	strings: StringTableBuilder,
	software_lists: BTreeMap<String, SoftwareListBuild>,
	ram_options: BinBuilder<binary::RamOption>,
	build_strindex: u32,
	phase_specific: Option<PhaseSpecificState>,
}

enum PhaseSpecificState {
	Extensions(String),
	RamOption(bool),
}

#[derive(Debug, Default)]
struct SoftwareListBuild {
	pub originals: Vec<u32>,
	pub compatibles: Vec<u32>,
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Missing mandatory attribute {0} when parsing InfoDB")]
	MissingMandatoryAttribute(&'static str),
}

impl State {
	pub fn new() -> Self {
		// prepare a string table, allocating capacity with respect to what we know about MAME 0.239
		let mut strings = StringTableBuilder::new(4500000); // 4326752 bytes

		// placeholder build string, which will be overridden later on
		let build_strindex = strings.lookup("");

		// reserve space based the same MAME version as above
		Self {
			phase_stack: Vec::with_capacity(32),
			machines: BinBuilder::new(48000),              // 44092 machines,
			chips: BinBuilder::new(190000),                // 174679 chips
			devices: BinBuilder::new(12000),               // 10738 devices
			slots: BinBuilder::new(1000),                  // ??? slots
			slot_options: BinBuilder::new(1000),           // ??? slot options
			machine_software_lists: BinBuilder::new(6800), // 6337 software lists
			ram_options: BinBuilder::new(6800),            // 6383 ram options
			software_lists: BTreeMap::new(),
			strings,
			build_strindex,
			phase_specific: None,
		}
	}

	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> Result<Option<Phase>> {
		event!(LOG, "handle_start(): self={:?}", self);
		event!(LOG, "handle_start(): {:?}", evt);

		let phase = self.phase_stack.last().unwrap_or(&Phase::Root);
		let new_phase = match (phase, evt.name().as_ref()) {
			(Phase::Root, b"mame") => {
				let [build] = evt.find_attributes([b"build"])?;
				self.build_strindex = self.strings.lookup(&build.unwrap_or_default());
				Some(Phase::Mame)
			}
			(Phase::Mame, b"machine") => {
				let [name, source_file, clone_of, rom_of, runnable] =
					evt.find_attributes([b"name", b"sourcefile", b"cloneof", b"romof", b"runnable"])?;

				event!(
					LOG,
					"handle_start(): name={:?} source_file={:?} runnable={:?}",
					name,
					source_file,
					runnable
				);

				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?;
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
					chips_start: self.chips.len(),
					chips_end: self.chips.len(),
					devices_start: self.devices.len(),
					devices_end: self.devices.len(),
					slots_start: self.slots.len(),
					slots_end: self.slots.len(),
					machine_software_lists_start: self.machine_software_lists.len(),
					machine_software_lists_end: self.machine_software_lists.len(),
					ram_options_start: self.ram_options.len(),
					ram_options_end: self.ram_options.len(),
					runnable,
					..Default::default()
				};
				self.machines.push(machine);
				Some(Phase::Machine)
			}
			(Phase::Machine, b"description") => Some(Phase::MachineDescription),
			(Phase::Machine, b"year") => Some(Phase::MachineYear),
			(Phase::Machine, b"manufacturer") => Some(Phase::MachineManufacturer),
			(Phase::Machine, b"chip") => {
				let [chip_type, tag, name, clock] = evt.find_attributes([b"type", b"tag", b"name", b"clock"])?;
				let Ok(chip_type) = chip_type
					.ok_or(ThisError::MissingMandatoryAttribute("type"))?
					.as_ref()
					.parse::<ChipType>()
				else {
					// presumably an unknown chip type; ignore
					return Ok(None);
				};
				let tag_strindex = self.strings.lookup(&tag.unwrap_or_default());
				let name_strindex = self.strings.lookup(&name.unwrap_or_default());
				let clock = clock.as_ref().and_then(|x| x.parse().ok()).unwrap_or(0);
				let chip = binary::Chip {
					chip_type,
					tag_strindex,
					name_strindex,
					clock,
				};
				self.chips.push(chip);
				self.machines.increment(|m| &mut m.chips_end)?;
				None
			}
			(Phase::Machine, b"device") => {
				let [device_type, tag, mandatory, interface] =
					evt.find_attributes([b"type", b"tag", b"mandatory", b"interface"])?;
				let tag = tag.ok_or(ThisError::MissingMandatoryAttribute("tag"))?;
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
					extensions_strindex: 0,
				};
				self.devices.push(device);
				self.machines.increment(|m| &mut m.devices_end)?;
				self.phase_specific = Some(PhaseSpecificState::Extensions(String::with_capacity(1024)));
				Some(Phase::MachineDevice)
			}
			(Phase::Machine, b"slot") => {
				let [name] = evt.find_attributes([b"name"])?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("slot"))?;
				let name = normalize_tag(name);
				let name_strindex: u32 = self.strings.lookup(&name);
				let slot = binary::Slot {
					name_strindex,
					options_start: self.slot_options.len(),
					options_end: self.slot_options.len(),
					default_option_index: !0,
				};
				self.slots.push(slot);
				self.machines.increment(|m| &mut m.slots_end)?;
				Some(Phase::MachineSlot)
			}
			(Phase::Machine, b"softwarelist") => {
				let [tag, name, status, filter] = evt.find_attributes([b"tag", b"name", b"status", b"filter"])?;
				let status = status.ok_or(ThisError::MissingMandatoryAttribute("status"))?;
				let Ok(status) = status.as_ref().parse::<SoftwareListStatus>() else {
					// presumably an unknown software list status; ignore
					return Ok(None);
				};
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?;
				let tag_strindex = self.strings.lookup(&tag.unwrap_or_default());
				let name_strindex = self.strings.lookup(&name);
				let filter_strindex = self.strings.lookup(&filter.unwrap_or_default());
				let machine_software_list = binary::MachineSoftwareList {
					tag_strindex,
					software_list_index: name_strindex,
					status,
					filter_strindex,
				};
				self.machine_software_lists.push(machine_software_list);
				self.machines.increment(|m| &mut m.machine_software_lists_end)?;

				// add this machine to the global software list
				let software_list = self.software_lists.entry(name.to_string()).or_default();
				let list = match status {
					SoftwareListStatus::Original => &mut software_list.originals,
					SoftwareListStatus::Compatible => &mut software_list.compatibles,
				};
				list.push(self.machines.items().next_back().unwrap().name_strindex);
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
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?;
				let devname = devname.ok_or(ThisError::MissingMandatoryAttribute("devname"))?;
				let name_strindex = self.strings.lookup(&name);
				let devname_strindex = self.strings.lookup(&devname);
				let is_default = is_default.map(parse_mame_bool).transpose()?.unwrap_or_default();
				if is_default {
					self.slots
						.tweak(|s| s.default_option_index = s.options_end - s.options_start);
				}
				let slot_option = binary::SlotOption {
					name_strindex,
					devname_strindex,
				};
				self.slots.increment(|s| &mut s.options_end)?;
				self.slot_options.push(slot_option);
				None
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(&mut self, callback: &mut impl FnMut(&str) -> bool, text: Option<String>) -> Result<Option<()>> {
		event!(LOG, "handle_end(): self={:?}", self);

		match self.phase_stack.last().unwrap_or(&Phase::Root) {
			Phase::MachineDescription => {
				let description = text.unwrap();
				if !description.is_empty() && callback(&description) {
					return Ok(None);
				}
				let description_strindex = self.strings.lookup(&description);
				self.machines.tweak(|x| x.description_strindex = description_strindex);
			}
			Phase::MachineYear => {
				let year_strindex = self.strings.lookup(&text.unwrap());
				self.machines.tweak(|x| x.year_strindex = year_strindex);
			}
			Phase::MachineManufacturer => {
				let manufacturer_strindex = self.strings.lookup(&text.unwrap());
				self.machines.tweak(|x| x.manufacturer_strindex = manufacturer_strindex);
			}
			Phase::MachineDevice => {
				let PhaseSpecificState::Extensions(extensions) = self.phase_specific.take().unwrap() else {
					unreachable!();
				};
				let extensions = extensions.split('\0').sorted().join("\0");
				let extensions_strindex = self.strings.lookup(&extensions);
				self.devices.tweak(|d| d.extensions_strindex = extensions_strindex);
			}
			Phase::MachineRamOption => {
				let PhaseSpecificState::RamOption(is_default) = self.phase_specific.take().unwrap() else {
					unreachable!();
				};
				if let Ok(size) = text.unwrap().parse::<u64>() {
					let ram_option = binary::RamOption { size, is_default };
					self.ram_options.push(ram_option);
					self.machines.increment(|x| &mut x.ram_options_end)?;
				}
			}
			_ => {}
		};
		Ok(Some(()))
	}

	pub fn into_data(mut self) -> Result<Box<[u8]>> {
		// canonicalize name_strindex, so we don't have both inline and indexed
		// small sting references
		self.machines
			.tweak_all(|machine| {
				let old_strindex = machine.name_strindex;
				let new_strindex = self
					.strings
					.lookup_immut(&self.strings.index(old_strindex))
					.unwrap_or(old_strindex);
				machine.name_strindex = new_strindex;
				Ok::<(), ()>(())
			})
			.unwrap();

		// sort machines
		self.machines.sort_by(|a, b| {
			let a = self.strings.index(a.name_strindex);
			let b = self.strings.index(b.name_strindex);
			a.cmp(&b)
		});

		// build a "machine.name_strindex" ==> "machine_index" map in preparations for fixups
		let machines_indexmap = self
			.machines
			.items()
			.enumerate()
			.map(|(index, obj)| (obj.name_strindex, u32::try_from(index).unwrap()))
			.collect::<HashMap<_, _>>();
		let machine_count = self.machines.len();
		let machines_indexmap = |strindex| {
			let result = machines_indexmap
				.get(&strindex)
				.or_else(|| {
					let new_strindex = self.strings.lookup_immut(&self.strings.index(strindex));
					new_strindex.and_then(|x| machines_indexmap.get(&x))
				})
				.copied();

			// sanity check and return
			assert!(result.is_none_or(|x| x < machine_count), "Invalid machine");
			result
		};

		// software lists require special processing
		let mut software_list_machine_indexes = BinBuilder::<u32>::new(0);
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
				let index = u32::try_from(index).unwrap();
				let name_strindex = self.strings.lookup_immut(&software_list_name).unwrap();
				let software_list_original_machines_start = software_list_machine_indexes.len();
				let software_list_compatible_machines_start =
					software_list_original_machines_start + u32::try_from(originals.len()).unwrap();
				let software_list_compatible_machines_end =
					software_list_compatible_machines_start + u32::try_from(compatibles.len()).unwrap();
				let entry = binary::SoftwareList {
					name_strindex,
					software_list_original_machines_start,
					software_list_compatible_machines_start,
					software_list_compatible_machines_end,
				};
				assert!(originals.iter().all(|&x| x < self.machines.len()));
				assert!(compatibles.iter().all(|&x| x < self.machines.len()));

				software_list_machine_indexes.extend(originals);
				software_list_machine_indexes.extend(compatibles);
				software_list_indexmap.insert(name_strindex, index);
				entry
			})
			.collect::<BinBuilder<_>>();
		let software_list_indexmap = |software_list_index| {
			software_list_indexmap
				.get(&software_list_index)
				.copied()
				.or_else(|| self.strings.lookup_immut(&self.strings.index(software_list_index)))
				.unwrap()
		};

		// and run the fixups
		fixup(
			&mut self.machines,
			&self.strings,
			machines_indexmap,
			software_list_indexmap,
		)?;
		fixup(
			&mut self.machine_software_lists,
			&self.strings,
			machines_indexmap,
			software_list_indexmap,
		)?;

		// assemble the header
		let header = binary::Header {
			magic: *MAGIC_HDR,
			sizes_hash: calculate_sizes_hash(),
			build_strindex: self.build_strindex,
			machine_count: self.machines.len(),
			chips_count: self.chips.len(),
			device_count: self.devices.len(),
			slot_count: self.slots.len(),
			slot_option_count: self.slot_options.len(),
			software_list_count: software_lists.len(),
			software_list_machine_count: software_list_machine_indexes.len(),
			machine_software_lists_count: self.machine_software_lists.len(),
			ram_option_count: self.ram_options.len(),
		};
		let mut header_bytes = [0u8; binary::Header::SERIALIZED_SIZE];
		header.binary_serialize(&mut header_bytes, ENDIANNESS);

		// get all bytes and return
		let bytes = header_bytes
			.into_iter()
			.chain(self.machines.into_iter())
			.chain(self.chips.into_iter())
			.chain(self.devices.into_iter())
			.chain(self.slots.into_iter())
			.chain(self.slot_options.into_iter())
			.chain(software_lists.into_iter())
			.chain(software_list_machine_indexes.into_iter())
			.chain(self.machine_software_lists.into_iter())
			.chain(self.ram_options.into_iter())
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
	bin_builder: &mut BinBuilder<impl Fixup + BinarySerde>,
	strings: &StringTableBuilder,
	machines_indexmap: impl Fn(u32) -> Option<u32>,
	software_list_indexmap: impl Fn(u32) -> u32,
) -> Result<()> {
	bin_builder.tweak_all(|x| {
		for machine_index in x.identify_machine_indexes() {
			let new_machine_index = if *machine_index != 0 {
				machines_indexmap(*machine_index).ok_or_else(|| {
					let message = format!(
						"Bad machine reference in MAME -listxml output: {}",
						strings.index(*machine_index)
					);
					Error::msg(message)
				})?
			} else {
				!0
			};
			*machine_index = new_machine_index;
		}
		for software_list_index in x.identify_software_list_indexes() {
			*software_list_index = software_list_indexmap(*software_list_index);
		}
		Ok(())
	})
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
	mut callback: impl FnMut(&str) -> bool,
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

#[derive(Debug)]
struct BinBuilder<T>
where
	T: BinarySerde,
{
	vec: Vec<u8>,
	phantom_data: PhantomData<T>,
}

impl<T> BinBuilder<T>
where
	T: BinarySerde,
{
	fn new(capacity: usize) -> Self {
		Self {
			vec: Vec::with_capacity(capacity * T::SERIALIZED_SIZE),
			phantom_data: PhantomData,
		}
	}

	fn push(&mut self, obj: T) {
		let pos = self.vec.len();
		self.vec.resize(pos + T::SERIALIZED_SIZE, 0x00);
		obj.binary_serialize(&mut self.vec[pos..], ENDIANNESS);
	}

	fn tweak<R>(&mut self, func: impl FnOnce(&mut T) -> R) -> R {
		let index = (self.len() - 1).try_into().unwrap();
		self.tweak_by_index(index, func)
	}

	fn tweak_by_index<R>(&mut self, index: usize, func: impl FnOnce(&mut T) -> R) -> R {
		let pos = index * T::SERIALIZED_SIZE;
		let slice = &mut self.vec[pos..];
		let mut obj = T::binary_deserialize(slice, ENDIANNESS).unwrap();
		let result = func(&mut obj);
		obj.binary_serialize(slice, ENDIANNESS);
		result
	}

	fn tweak_all<E>(&mut self, func: impl Fn(&mut T) -> Result<(), E>) -> Result<(), E> {
		for slice in self.vec.chunks_mut(T::SERIALIZED_SIZE) {
			let mut obj = T::binary_deserialize(slice, ENDIANNESS).unwrap();
			func(&mut obj)?;
			obj.binary_serialize(slice, ENDIANNESS);
		}
		Ok(())
	}

	fn increment<N>(&mut self, func: impl FnOnce(&mut T) -> &mut N) -> Result<()>
	where
		N: CheckedAdd<Output = N> + Clone + From<u8> + Ord,
	{
		self.tweak(|x| {
			let value = func(x);
			if let Some(new_value) = (*value).clone().checked_add(&1.into()) {
				*value = new_value;
				Ok(())
			} else {
				Err(Error::msg("Overflow"))
			}
		})
	}

	fn len(&self) -> u32 {
		(self.vec.len() / T::SERIALIZED_SIZE).try_into().unwrap()
	}

	fn items(&self) -> impl DoubleEndedIterator<Item = T> + '_ {
		self.vec
			.chunks(T::SERIALIZED_SIZE)
			.map(|slice| T::binary_deserialize(slice, ENDIANNESS).unwrap())
	}

	fn into_iter(self) -> impl Iterator<Item = u8> {
		self.vec.into_iter()
	}

	fn sort_by(&mut self, cmp: impl Fn(T, T) -> Ordering) {
		let new_vec = self
			.vec
			.chunks(T::SERIALIZED_SIZE)
			.sorted_by(|slice_a, slice_b| {
				let obj_a = T::binary_deserialize(slice_a, ENDIANNESS).unwrap();
				let obj_b = T::binary_deserialize(slice_b, ENDIANNESS).unwrap();
				cmp(obj_a, obj_b)
			})
			.flat_map(|x| x.iter().cloned())
			.collect::<Vec<_>>();
		self.vec = new_vec;
	}
}

impl<T> FromIterator<T> for BinBuilder<T>
where
	T: BinarySerde,
{
	fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
		let iter = iter.into_iter();
		let (_, capacity) = iter.size_hint();
		let capacity = capacity.unwrap_or_default();
		let mut bin_builder = Self::new(capacity);
		for obj in iter {
			bin_builder.push(obj);
		}
		bin_builder
	}
}

impl<T> Extend<T> for BinBuilder<T>
where
	T: BinarySerde,
{
	fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
		for obj in iter {
			self.push(obj);
		}
	}
}

pub fn calculate_sizes_hash() -> u64 {
	let multiplicand = 4733; // arbitrary prime number
	[
		binary::Header::SERIALIZED_SIZE,
		binary::Machine::SERIALIZED_SIZE,
		binary::Chip::SERIALIZED_SIZE,
		binary::Device::SERIALIZED_SIZE,
		binary::Slot::SERIALIZED_SIZE,
		binary::SlotOption::SERIALIZED_SIZE,
		binary::SoftwareList::SERIALIZED_SIZE,
		binary::MachineSoftwareList::SERIALIZED_SIZE,
		binary::RamOption::SERIALIZED_SIZE,
	]
	.into_iter()
	.fold(0, |value, item| {
		u64::overflowing_mul(value, multiplicand).0 ^ (item as u64)
	})
}

#[cfg(test)]
mod test {
	use std::io::BufReader;

	use assert_matches::assert_matches;
	use test_case::test_case;

	use super::super::InfoDb;

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"))]
	#[test_case(1, include_str!("test_data/listxml_coco.xml"))]
	#[test_case(2, include_str!("test_data/listxml_fake.xml"))]
	pub fn data_from_listxml_output(_index: usize, xml: &str) {
		let reader = BufReader::new(xml.as_bytes());
		let data = super::data_from_listxml_output(reader, |_| false).unwrap().unwrap();
		let result = InfoDb::new(data);
		assert_matches!(result, Ok(_));
	}
}
