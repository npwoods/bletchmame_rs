use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::io::BufRead;
use std::marker::PhantomData;

use binary_serde::BinarySerde;
use itertools::Itertools;
use num::CheckedAdd;

use crate::error::BoxDynError;
use crate::info::binary;
use crate::info::binary::Fixup;
use crate::info::strings::StringTableBuilder;
use crate::info::ChipType;
use crate::info::SoftwareListStatus;
use crate::info::ENDIANNESS;
use crate::info::MAGIC_HDR;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;
use crate::Error;
use crate::Result;

const LOG: bool = false;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	Mame,
	Machine,
	MachineSubtag,
	MachineDescription,
	MachineYear,
	MachineManufacturer,
}

const TEXT_CAPTURE_PHASES: &[Phase] = &[
	Phase::MachineDescription,
	Phase::MachineYear,
	Phase::MachineManufacturer,
];

struct State {
	phase: Phase,
	machines: BinBuilder<binary::Machine>,
	chips: BinBuilder<binary::Chip>,
	machine_software_lists: BinBuilder<binary::MachineSoftwareList>,
	strings: StringTableBuilder,
	software_lists: BTreeMap<String, SoftwareListBuild>,
	build_strindex: u32,
}

#[derive(Debug, Default)]
struct SoftwareListBuild {
	pub originals: Vec<u32>,
	pub compatibles: Vec<u32>,
}

impl State {
	pub fn new() -> Self {
		// prepare a string table, allocating capacity with respect to what we know about MAME 0.239
		let mut strings = StringTableBuilder::new(4500000); // 4326752 bytes

		// placeholder build string, which will be overridden later on
		let build_strindex = strings.lookup("");

		// reserve space based the same MAME version as above
		Self {
			phase: Phase::Root,
			machines: BinBuilder::new(48000),              // 44092 machines,
			chips: BinBuilder::new(190000),                // 174679 chips
			machine_software_lists: BinBuilder::new(6800), // 6337 software lists
			software_lists: BTreeMap::new(),
			strings,
			build_strindex,
		}
	}

	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> std::result::Result<Option<Phase>, BoxDynError> {
		if LOG {
			println!("handle_start(): self={:?}", self);
			println!("handle_start(): {:?}", evt);
		}

		let new_phase = match (self.phase, evt.name().as_ref()) {
			(Phase::Root, b"mame") => {
				let [build] = evt.find_attributes([b"build"])?;
				self.build_strindex = self.strings.lookup(&build.unwrap_or_default());
				Some(Phase::Mame)
			}
			(Phase::Mame, b"machine") => {
				let [name, source_file, clone_of, rom_of, runnable] =
					evt.find_attributes([b"name", b"sourcefile", b"cloneof", b"romof", b"runnable"])?;

				if LOG {
					println!(
						"handle_start(): name={:?} source_file={:?} runnable={:?}",
						name, source_file, runnable
					);
				}

				let Some(name) = name else { return Ok(None) };
				let name_strindex = self.strings.lookup(&name);
				let source_file_strindex = self.strings.lookup(&source_file.unwrap_or_default());
				let clone_of_machine_index = self.strings.lookup(&clone_of.unwrap_or_default());
				let rom_of_machine_index = self.strings.lookup(&rom_of.unwrap_or_default());
				let runnable = runnable.map(bool_attribute).unwrap_or(true);
				let machine = binary::Machine {
					name_strindex,
					source_file_strindex,
					clone_of_machine_index,
					rom_of_machine_index,
					chips_start: self.chips.len(),
					chips_end: self.chips.len(),
					machine_software_lists_start: self.machine_software_lists.len(),
					machine_software_lists_end: self.machine_software_lists.len(),
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
				let Some(chip_type) = chip_type.and_then(|x| x.as_ref().parse::<ChipType>().ok()) else {
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
				Some(Phase::MachineSubtag)
			}
			(Phase::Machine, b"softwarelist") => {
				let [tag, name, status, filter] = evt.find_attributes([b"tag", b"name", b"status", b"filter"])?;
				let Some(status) = status.and_then(|x| x.as_ref().parse::<SoftwareListStatus>().ok()) else {
					return Ok(None);
				};
				let Some(name) = name else {
					return Ok(None);
				};
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
				Some(Phase::MachineSubtag)
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(
		&mut self,
		callback: &mut impl FnMut(&str) -> bool,
		text: Option<String>,
	) -> std::result::Result<Option<Phase>, BoxDynError> {
		if LOG {
			println!("handle_end(): self={:?}", self);
		}

		let new_phase = match self.phase {
			Phase::Root => panic!(),
			Phase::Mame => Phase::Root,
			Phase::Machine => Phase::Mame,
			Phase::MachineSubtag => Phase::Machine,

			Phase::MachineDescription => {
				let description = text.unwrap();
				if !description.is_empty() && callback(&description) {
					return Ok(None);
				}
				let description_strindex = self.strings.lookup(&description);
				self.machines.tweak(|x| x.description_strindex = description_strindex);
				Phase::Machine
			}
			Phase::MachineYear => {
				let year_strindex = self.strings.lookup(&text.unwrap());
				self.machines.tweak(|x| x.year_strindex = year_strindex);
				Phase::Machine
			}
			Phase::MachineManufacturer => {
				let manufacturer_strindex = self.strings.lookup(&text.unwrap());
				self.machines.tweak(|x| x.manufacturer_strindex = manufacturer_strindex);
				Phase::Machine
			}
		};
		Ok(Some(new_phase))
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
			assert!(!result.is_some_and(|x| x >= machine_count), "Invalid machine");
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
			software_list_count: software_lists.len(),
			software_list_machine_count: software_list_machine_indexes.len(),
			machine_software_lists_count: self.machine_software_lists.len(),
		};
		let mut header_bytes = [0u8; binary::Header::SERIALIZED_SIZE];
		header.binary_serialize(&mut header_bytes, ENDIANNESS);

		// get all bytes and return
		let bytes = header_bytes
			.into_iter()
			.chain(self.machines.into_iter())
			.chain(self.chips.into_iter())
			.chain(software_lists.into_iter())
			.chain(software_list_machine_indexes.into_iter())
			.chain(self.machine_software_lists.into_iter())
			.chain(self.strings.into_iter())
			.collect();
		Ok(bytes)
	}
}

impl Debug for State {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
		f.debug_struct("State")
			.field("phase", &self.phase)
			.field("machines.len()", &self.machines.len())
			.field("chips.len()", &self.chips.len())
			.finish()
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
					let bad_reference = strings.index(*machine_index).to_string();
					Error::BadMachineReference(bad_reference)
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

fn listxml_err(reader: &XmlReader<impl BufRead>, e: BoxDynError) -> crate::error::Error {
	Error::ListXmlProcessing(reader.buffer_position(), e)
}

pub fn data_from_listxml_output(
	reader: impl BufRead,
	mut callback: impl FnMut(&str) -> bool,
) -> Result<Option<Box<[u8]>>> {
	let mut state = State::new();
	let mut reader = XmlReader::from_reader(reader);
	let mut buf = Vec::with_capacity(1024);

	while let Some(evt) = reader.next(&mut buf).map_err(|e| listxml_err(&reader, e))? {
		match evt {
			XmlEvent::Start(evt) => {
				let new_phase = state.handle_start(evt).map_err(|e| listxml_err(&reader, e))?;

				if let Some(new_phase) = new_phase {
					state.phase = new_phase;

					if TEXT_CAPTURE_PHASES.contains(&state.phase) {
						reader.start_text_capture();
					}
				} else {
					reader.start_unknown_tag();
				}
			}

			XmlEvent::End(s) => {
				let new_phase = state
					.handle_end(&mut callback, s)
					.map_err(|e| listxml_err(&reader, e))?;
				let Some(new_phase) = new_phase else {
					return Ok(None);
				};
				state.phase = new_phase;
			}

			XmlEvent::Null => {} // meh
		}
	}

	// sanity check
	assert_eq!(Phase::Root, state.phase);

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

	fn tweak_all<E>(&mut self, func: impl Fn(&mut T) -> std::result::Result<(), E>) -> std::result::Result<(), E> {
		for slice in self.vec.chunks_mut(T::SERIALIZED_SIZE) {
			let mut obj = T::binary_deserialize(slice, ENDIANNESS).unwrap();
			func(&mut obj)?;
			obj.binary_serialize(slice, ENDIANNESS);
		}
		Ok(())
	}

	fn increment<N>(&mut self, func: impl FnOnce(&mut T) -> &mut N) -> std::result::Result<(), BoxDynError>
	where
		N: CheckedAdd<Output = N> + Clone + From<u8> + Ord,
	{
		self.tweak(|x| {
			let value = func(x);
			if let Some(new_value) = (*value).clone().checked_add(&1.into()) {
				*value = new_value;
				Ok(())
			} else {
				Err(BoxDynError::from("Overflow"))
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

fn bool_attribute(text: impl AsRef<str>) -> bool {
	let text = text.as_ref();
	text == "yes" || text == "1" || text == "true"
}

pub fn calculate_sizes_hash() -> u64 {
	let multiplicand = 4729; // arbitrary prime number
	[
		binary::Header::SERIALIZED_SIZE,
		binary::Machine::SERIALIZED_SIZE,
		binary::Chip::SERIALIZED_SIZE,
		binary::SoftwareList::SERIALIZED_SIZE,
		binary::MachineSoftwareList::SERIALIZED_SIZE,
	]
	.into_iter()
	.fold(0, |value, item| (value * multiplicand) ^ (item as u64))
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
