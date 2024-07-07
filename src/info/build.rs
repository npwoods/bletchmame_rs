use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::fmt::Write;
use std::io::BufRead;
use std::marker::PhantomData;
use std::str::from_utf8;

use binary_serde::BinarySerde;
use itertools::Itertools;
use num::CheckedAdd;
use quick_xml::escape::unescape;
use quick_xml::events::BytesStart;
use quick_xml::events::BytesText;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::BoxDynError;
use crate::info::binary;
use crate::info::strings::StringTableBuilder;
use crate::info::ChipType;
use crate::info::ENDIANNESS;
use crate::info::MAGIC_HDR;
use crate::Error;
use crate::Result;

const LOG: bool = true;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
	Root,
	Mame,
	Machine,
	MachineSubtag,
	MachineDescription,
	MachineYear,
	MachineManufacturer,
}

struct State {
	phase: Phase,
	machines: BinBuilder<binary::Machine>,
	chips: BinBuilder<binary::Chip>,
	strings: StringTableBuilder,
	current_text: Option<String>,
	build_strindex: u32,
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
			machines: BinBuilder::new(48000), // 44092 machines,
			chips: BinBuilder::new(190000),   // 174679 chips
			strings,
			current_text: None,
			build_strindex,
		}
	}

	pub fn handle_start(&mut self, evt: BytesStart) -> std::result::Result<Option<Phase>, BoxDynError> {
		if LOG {
			println!("handle_start(): self={:?}", self);
			println!("handle_start(): {}", debug_start_tag(&evt));
		}

		let new_phase = match (self.phase, evt.name().as_ref()) {
			(Phase::Root, b"mame") => {
				let [build] = find_attributes(&evt, [b"build"]);
				self.build_strindex = self.strings.lookup(&build.unwrap_or_default());
				Some(Phase::Mame)
			}
			(Phase::Mame, b"machine") => {
				let [name, source_file, runnable] = find_attributes(&evt, [b"name", b"sourcefile", b"runnable"]);
				let Some(name) = name else { return Ok(None) };
				let name_strindex = self.strings.lookup(&name);
				let source_file_strindex = self.strings.lookup(&source_file.unwrap_or_default());
				let runnable = runnable.map(bool_attribute).unwrap_or(true);
				let machine = binary::Machine {
					name_strindex,
					source_file_strindex,
					chips_index: self.chips.len(),
					runnable,
					..Default::default()
				};
				self.machines.push(machine);
				Some(Phase::Machine)
			}
			(Phase::Machine, b"description") => {
				self.current_text = Some(String::new());
				Some(Phase::MachineDescription)
			}
			(Phase::Machine, b"year") => {
				self.current_text = Some(String::new());
				Some(Phase::MachineYear)
			}
			(Phase::Machine, b"manufacturer") => {
				self.current_text = Some(String::new());
				Some(Phase::MachineManufacturer)
			}
			(Phase::Machine, b"chip") => {
				let [chip_type, tag, name, clock] = find_attributes(&evt, [b"type", b"tag", b"name", b"clock"]);
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
				self.machines.increment(|m| &mut m.chips_count)?;
				Some(Phase::MachineSubtag)
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(
		&mut self,
		callback: &mut impl FnMut(&str) -> bool,
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
				let description = self.current_text.take().unwrap_or_default();
				if !description.is_empty() && callback(&description) {
					return Ok(None);
				}
				let description_strindex = self.strings.lookup(&description);
				self.machines.tweak(|x| x.description_strindex = description_strindex);
				Phase::Machine
			}
			Phase::MachineYear => {
				let year_strindex = self.strings.lookup(&self.current_text.take().unwrap_or_default());
				self.machines.tweak(|x| x.year_strindex = year_strindex);
				Phase::Machine
			}
			Phase::MachineManufacturer => {
				let manufacturer_strindex = self.strings.lookup(&self.current_text.take().unwrap_or_default());
				self.machines.tweak(|x| x.manufacturer_strindex = manufacturer_strindex);
				Phase::Machine
			}
		};
		Ok(Some(new_phase))
	}

	pub fn into_data(mut self) -> Box<[u8]> {
		// sort machines
		self.machines.sort_by(|a, b| {
			let a = self.strings.index(a.name_strindex);
			let b = self.strings.index(b.name_strindex);
			a.cmp(&b)
		});

		// assemble the header
		let header = binary::Header {
			magic: *MAGIC_HDR,
			sizes_hash: calculate_sizes_hash(),
			build_strindex: self.build_strindex,
			machine_count: self.machines.len(),
			chips_count: self.chips.len(),
		};
		let mut header_bytes: [u8; binary::Header::SERIALIZED_SIZE] = Default::default();
		header.binary_serialize(&mut header_bytes, ENDIANNESS);

		// get all bytes and return
		header_bytes
			.into_iter()
			.chain(self.machines.into_iter())
			.chain(self.chips.into_iter())
			.chain(self.strings.into_iter())
			.collect()
	}
}

impl Debug for State {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
		write!(
			f,
			"[phase={:?} machine.len()={} current_text={:?}]",
			self.phase,
			self.machines.len(),
			self.current_text
		)
	}
}

fn listxml_err<R>(reader: &quick_xml::Reader<R>, e: BoxDynError) -> crate::error::Error {
	Error::ListXmlProcessing(reader.buffer_position(), e)
}

/// quick-xml events are at a slightly different granularity than what we would prefer
enum XmlEvent<'a> {
	Start(BytesStart<'a>),
	End,
	Text(BytesText<'a>),
}

pub fn data_from_listxml_output(
	reader: impl BufRead,
	mut callback: impl FnMut(&str) -> bool,
) -> Result<Option<Box<[u8]>>> {
	let mut state = State::new();
	let mut reader = Reader::from_reader(reader);
	let mut buf = Vec::new();
	let mut unknown_depth = 0;

	loop {
		// read the quick-xml event
		let qxml_event = reader
			.read_event_into(&mut buf)
			.map_err(|e| listxml_err(&reader, e.into()))?;

		// convert to our events
		let events = match qxml_event {
			Event::Eof => {
				assert_eq!(Phase::Root, state.phase);
				break;
			}
			Event::Start(x) => vec![XmlEvent::Start(x)],
			Event::End(_) => vec![XmlEvent::End],
			Event::Empty(x) => vec![XmlEvent::Start(x), XmlEvent::End],
			Event::Text(x) => vec![XmlEvent::Text(x)],
			_ => vec![],
		};

		// and loop through them
		for evt in events {
			match evt {
				XmlEvent::Start(evt) => {
					let new_phase = (unknown_depth == 0)
						.then(|| state.handle_start(evt))
						.transpose()
						.map_err(|e| listxml_err(&reader, e))?
						.flatten();

					if let Some(new_phase) = new_phase {
						state.phase = new_phase
					} else {
						unknown_depth += 1
					}
				}

				XmlEvent::End => {
					if unknown_depth == 0 {
						let new_phase = state.handle_end(&mut callback).map_err(|e| listxml_err(&reader, e))?;
						let Some(new_phase) = new_phase else {
							return Ok(None);
						};
						state.phase = new_phase;
					} else {
						unknown_depth -= 1;
					}
				}

				XmlEvent::Text(bytes_text) => {
					if let Some(current_text) = &mut state.current_text {
						let string = cow_bytes_to_str(bytes_text.into_inner()).map_err(|e| listxml_err(&reader, e))?;
						current_text.push_str(&string);
					}
				}
			}
		}
	}

	// get all bytes and return
	let data = state.into_data();
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

fn find_attributes<'a, const N: usize>(e: &'a BytesStart, attrs: [&[u8]; N]) -> [Option<Cow<'a, str>>; N] {
	attrs
		.iter()
		.map(|&attr_name| {
			e.attributes()
				.filter_map(|x| x.ok())
				.find(|x| x.key.as_ref() == attr_name)
				.and_then(|x| cow_bytes_to_str(x.value).ok())
		})
		.collect::<Vec<_>>()
		.try_into()
		.unwrap()
}

fn bool_attribute(text: impl AsRef<str>) -> bool {
	let text = text.as_ref();
	text == "yes" || text == "1" || text == "true"
}

fn cow_bytes_to_str(cow: Cow<'_, [u8]>) -> std::result::Result<Cow<'_, str>, Box<dyn std::error::Error + Send + Sync>> {
	match cow {
		Cow::Borrowed(bytes) => {
			let s = from_utf8(bytes)?;
			Ok(unescape(s)?)
		}
		Cow::Owned(bytes) => {
			let s = from_utf8(&bytes)?;
			let s = unescape(s)?;
			Ok(s.into_owned().into())
		}
	}
}

pub fn calculate_sizes_hash() -> u64 {
	let multiplicand = 4729; // arbitrary prime number
	[
		binary::Header::SERIALIZED_SIZE,
		binary::Machine::SERIALIZED_SIZE,
		binary::Chip::SERIALIZED_SIZE,
	]
	.into_iter()
	.fold(0, |value, item| (value * multiplicand) ^ (item as u64))
}

fn debug_start_tag(e: &BytesStart) -> String {
	let mut text = String::with_capacity(1024);
	write!(text, "<{}", String::from_utf8_lossy(e.name().as_ref())).unwrap();
	for x in e.attributes().with_checks(false) {
		let attribute = x.unwrap();
		write!(
			text,
			" {}=\"{}\"",
			String::from_utf8_lossy(attribute.key.as_ref()),
			String::from_utf8_lossy(attribute.value.as_ref())
		)
		.unwrap();
	}
	write!(text, ">").unwrap();
	text
}

#[cfg(test)]
mod test {
	use std::borrow::Cow;
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

	#[test_case(0, Cow::Borrowed(b""), Ok(""))]
	#[test_case(1, Cow::Owned(b"".into()), Ok(""))]
	#[test_case(2, Cow::Borrowed(b"foo"), Ok("foo"))]
	#[test_case(3, Cow::Owned(b"foo".into()), Ok("foo"))]
	#[test_case(4, Cow::Borrowed(b"&lt;escaping&gt; &amp; things"), Ok("<escaping> & things"))]
	#[test_case(5, Cow::Owned(b"&lt;escaping&gt; &amp; things".into()), Ok("<escaping> & things"))]
	pub fn cow_bytes_to_str(_index: usize, input: Cow<'_, [u8]>, expected: std::result::Result<&str, ()>) {
		let actual = super::cow_bytes_to_str(input);
		let actual = actual.as_ref().map_or_else(|_| Err(()), |x| Ok(x.as_ref()));
		assert_eq!(expected, actual);
	}
}
