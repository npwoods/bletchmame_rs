use std::borrow::Cow;
use std::cmp::Ordering;
use std::io::BufRead;
use std::marker::PhantomData;
use std::str::from_utf8;

use binary_serde::BinarySerde;
use itertools::Itertools;
use quick_xml::escape::unescape;
use quick_xml::events::BytesStart;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::info::binary;
use crate::info::strings::StringTableBuilder;
use crate::info::ENDIANNESS;
use crate::info::MAGIC_HDR;
use crate::Error;
use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
	Root,
	Mame,
	Machine,
	MachineDescription,
	MachineYear,
	MachineManufacturer,
}

pub fn data_from_listxml_output(
	reader: impl BufRead,
	mut callback: impl FnMut(&str) -> bool,
) -> Result<Option<Box<[u8]>>> {
	let mut state = State::Root;
	let mut reader = Reader::from_reader(reader);
	let mut buf = Vec::new();
	let mut unknown_depth = 0;
	let mut current_text = None;

	// reserve space based on what we know about MAME 0.239
	let mut machines = BinBuilder::new(48000); // 44092 machines
	let mut strings = StringTableBuilder::new(4500000); // 4326752 bytes

	// root level attributes
	let mut build_strindex = strings.lookup("");

	loop {
		let evt = reader
			.read_event_into(&mut buf)
			.map_err(|e| Error::ListXmlProcessing(reader.buffer_position(), e.into()))?;
		match evt {
			Event::Eof => {
				assert_eq!(State::Root, state);
				break;
			}

			Event::Start(e) => {
				let new_state = (unknown_depth == 0)
					.then(|| match (state, e.name().as_ref()) {
						(State::Root, b"mame") => {
							let [build] = find_attributes(&e, [b"build"]);
							build_strindex = strings.lookup(&build.unwrap_or_default());
							Some(State::Mame)
						}
						(State::Mame, b"machine") => {
							let [name, source_file, runnable] =
								find_attributes(&e, [b"name", b"sourcefile", b"runnable"]);
							let name_strindex = strings.lookup(&name.unwrap_or_default());
							let source_file_strindex = strings.lookup(&source_file.unwrap_or_default());
							let runnable = runnable.map(bool_attribute).unwrap_or(true);
							let machine = binary::Machine {
								name_strindex,
								source_file_strindex,
								runnable,
								..Default::default()
							};
							machines.push(machine);
							Some(State::Machine)
						}
						(State::Machine, b"description") => {
							current_text = Some(String::new());
							Some(State::MachineDescription)
						}
						(State::Machine, b"year") => {
							current_text = Some(String::new());
							Some(State::MachineYear)
						}
						(State::Machine, b"manufacturer") => {
							current_text = Some(String::new());
							Some(State::MachineManufacturer)
						}
						_ => None,
					})
					.flatten();

				if let Some(new_state) = new_state {
					state = new_state
				} else {
					unknown_depth += 1
				}
			}

			Event::End(_) => {
				if unknown_depth == 0 {
					state = match state {
						State::Root => panic!(),
						State::Mame => State::Root,
						State::Machine => State::Mame,

						State::MachineDescription => {
							let description = current_text.take().unwrap_or_default();
							if !description.is_empty() && callback(&description) {
								return Ok(None);
							}
							let description_strindex = strings.lookup(&description);
							machines.tweak(|x| x.description_strindex = description_strindex);
							State::Machine
						}
						State::MachineYear => {
							let year_strindex = strings.lookup(&current_text.take().unwrap_or_default());
							machines.tweak(|x| x.year_strindex = year_strindex);
							State::Machine
						}
						State::MachineManufacturer => {
							let manufacturer_strindex = strings.lookup(&current_text.take().unwrap_or_default());
							machines.tweak(|x| x.manufacturer_strindex = manufacturer_strindex);
							State::Machine
						}
					};
				} else {
					unknown_depth -= 1;
				}
			}

			Event::Text(bytes_text) => {
				if let Some(current_text) = &mut current_text {
					let string = cow_bytes_to_str(bytes_text.into_inner())
						.map_err(|e| Error::ListXmlProcessing(reader.buffer_position(), e.into()))?;
					current_text.push_str(&string);
				}
			}

			// catch all
			_ => {}
		};
	}

	// sort machines
	machines.sort_by(|a, b| {
		let a = strings.index(a.name_strindex);
		let b = strings.index(b.name_strindex);
		a.cmp(&b)
	});

	// assemble the header
	let header = binary::Header {
		magic: *MAGIC_HDR,
		sizes_hash: calculate_sizes_hash(),
		build_strindex,
		machine_count: machines.len(),
	};
	let mut header_bytes: [u8; binary::Header::SERIALIZED_SIZE] = Default::default();
	header.binary_serialize(&mut header_bytes, ENDIANNESS);

	// get all bytes and return
	let data = header_bytes
		.into_iter()
		.chain(machines.into_iter())
		.chain(strings.into_iter())
		.collect();
	Ok(Some(data))
}

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

	fn tweak(&mut self, func: impl FnOnce(&mut T)) {
		let pos = self.vec.len() - T::SERIALIZED_SIZE;
		let slice = &mut self.vec[pos..];
		let mut obj = T::binary_deserialize(slice, ENDIANNESS).unwrap();
		func(&mut obj);
		obj.binary_serialize(slice, ENDIANNESS);
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
	[binary::Header::SERIALIZED_SIZE, binary::Machine::SERIALIZED_SIZE]
		.into_iter()
		.fold(0, |value, item| (value * multiplicand) ^ (item as u64))
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
