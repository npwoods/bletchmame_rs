use std::collections::HashMap;
use std::hash::DefaultHasher;
use std::hash::Hasher;

use smallvec::SmallVec;

use super::smallstr::SmallStrRef;

const SMALL_STRING_CHARS: &[u8; 63] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 ";

const MAGIC_STRINGTABLE_BEGIN: &[u8; 2] = &[0x9D, 0x9B];
const MAGIC_STRINGTABLE_END: &[u8; 2] = &[0x9F, 0x99];

#[derive(Debug)]
pub struct StringTableBuilder {
	data: Vec<u8>,
	map: HashMap<u64, SmallVec<[u32; 4]>>,
}

impl StringTableBuilder {
	pub fn new(capacity: usize) -> Self {
		// create the Vec that we write strings into
		let mut data = Vec::with_capacity(capacity);
		data.extend(MAGIC_STRINGTABLE_BEGIN.iter());

		// create the HashMap that we use as a cache
		let map = HashMap::with_capacity(capacity / 8);

		// seed the new StringTableBuilder with a value for empty strings
		let mut result = Self { data, map };
		result.map_insert("", 0);

		// and return
		result
	}

	pub fn lookup(&mut self, s: &str) -> u32 {
		self.map_lookup(s)
			.or_else(|| lookup_small(s.as_bytes()))
			.unwrap_or_else(|| {
				if !self.data.len() > MAGIC_STRINGTABLE_BEGIN.len() {
					self.data.push(0x80);
				}
				let result = self.data.len().try_into().unwrap();
				self.data.extend(s.as_bytes().iter());

				for (pos, _) in s.char_indices() {
					let element = result + u32::try_from(pos).unwrap();
					self.map_insert(&s[pos..], element);
				}
				result
			})
	}

	pub fn index(&self, offset: u32) -> SmallStrRef<'_> {
		read_string(&self.data, offset).unwrap()
	}

	pub fn into_iter(mut self) -> impl Iterator<Item = u8> {
		self.data.extend(MAGIC_STRINGTABLE_END.iter());
		self.data.into_iter()
	}

	fn map_lookup(&self, s: &str) -> Option<u32> {
		self.map
			.get(&hash(s))?
			.iter()
			.copied()
			.find(|&offset| s == self.index(offset))
	}

	fn map_insert(&mut self, s: &str, element: u32) {
		let key = hash(s);
		let entry = self.map.entry(key).or_default();

		let has_str = entry
			.iter()
			.copied()
			.any(|offset| s == read_string(&self.data, offset).unwrap());

		if !has_str {
			entry.push(element);
		}
	}
}

fn lookup_small(s: &[u8]) -> Option<u32> {
	(s.len() <= 5)
		.then_some((0..5).try_fold(0xC0000000, |acc, index| {
			let value = if let Some(&b) = s.get(index) {
				SMALL_STRING_CHARS.iter().position(|&x| x == b)?
			} else {
				SMALL_STRING_CHARS.len()
			};
			Some(acc | (value << (index * 6)) as u32)
		}))
		.flatten()
}

pub fn read_string(data: &[u8], offset: u32) -> std::result::Result<SmallStrRef<'_>, ()> {
	let result = if (offset & 0xC0000000) == 0xC0000000 {
		let iter = (0..5)
			.filter_map(|i| SMALL_STRING_CHARS.get(((offset >> (i * 6)) & 0x3F) as usize))
			.map(|&x| char::from_u32(x as u32).unwrap());
		SmallStrRef::from_small_chars(iter)
	} else {
		let offset = offset as usize;
		let data = data.get(offset..).ok_or(())?;
		data.utf8_chunks().next().map(|x| x.valid()).unwrap_or_default().into()
	};
	Ok(result)
}

pub fn validate_string_table(data: &[u8]) -> std::result::Result<(), ()> {
	if data.get(..MAGIC_STRINGTABLE_BEGIN.len()) != Some(MAGIC_STRINGTABLE_BEGIN) {
		return Err(());
	}
	if data.get((data.len() - MAGIC_STRINGTABLE_END.len())..) != Some(MAGIC_STRINGTABLE_END) {
		return Err(());
	}

	let middle_data = &data[(MAGIC_STRINGTABLE_BEGIN.len())..(data.len() - MAGIC_STRINGTABLE_END.len())];
	middle_data
		.utf8_chunks()
		.all(|chunk| chunk.invalid().is_empty() || chunk.invalid() == [0x80])
		.then_some(())
		.ok_or(())
}

fn hash(s: &str) -> u64 {
	let mut hasher = DefaultHasher::new();
	hasher.write(s.as_bytes());
	hasher.finish()
}

#[cfg(test)]
mod test {
	use assert_matches::assert_matches;
	use itertools::Itertools;
	use test_case::test_case;

	use crate::info::SmallStrRef;

	use super::StringTableBuilder;

	#[test_case(0, &[""])]
	#[test_case(1, &["Tiny"])]
	#[test_case(2, &["ReallyReallyLarge"])]
	#[test_case(3, &["", "Foo", "Bar", "Baz"])]
	#[test_case(4, &["", "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot"])]
	#[test_case(5, &["Whatchamakallit", "Thingamajig", "0123456789", "01234_56789"])]
	#[test_case(6, &["\0", " ", "Foo\0Bar", "Foo\u{1F4A9}Bar"])]
	pub fn test(_index: usize, strings: &[&str]) {
		let mut builder = StringTableBuilder::new(0);
		let strindexes = strings.iter().map(|&s| builder.lookup(s)).collect::<Vec<_>>();
		let string_table_bytes = builder.into_iter().collect::<Vec<_>>();
		let actual = strindexes
			.iter()
			.map(|&strindex| super::read_string(&string_table_bytes, strindex).unwrap())
			.collect::<Vec<_>>();
		assert_eq!(strings, actual);
	}

	#[test]
	pub fn empty_is_zero() {
		let mut builder = StringTableBuilder::new(0);
		let actual = builder.lookup("");
		assert_eq!(0, actual);
	}

	#[test_case(0, 0, Ok(""), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(1, 2, Ok("foo"), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(2, 6, Ok("bar"), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(3, 10, Ok(""), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(4, 11, Ok(""), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(5, 4242, Err(()), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	pub fn read_string(_index: usize, offset: u32, expected: std::result::Result<&str, ()>, bytes: &[u8]) {
		let expected = expected.map(String::from);
		let actual = super::read_string(bytes, offset);
		let actual = actual.map(String::from);
		assert_eq!(expected, actual);
	}

	#[test_case(0, "FooBarBaz", &["BarBaz", "Baz"])]
	#[test_case(1, "AlphaBravoCharlie", &["BravoCharlie", "Charlie"])]
	pub fn partial_strings(_index: usize, initial: &str, others: &[&str]) {
		let mut builder = StringTableBuilder::new(0);
		let _ = builder.lookup(initial);
		let len = builder.data.len();

		for other in others {
			let idx = builder.lookup(other);
			let result = builder.index(idx);

			assert_matches!(result, SmallStrRef::Ref(_));
			assert_eq!((*other, len), (result.as_ref(), builder.data.len()));
		}
	}

	#[test_case(0, &["?!", "!!"], &["", "!", "!!", "?!"])]
	pub fn no_dupes_in_map(_index: usize, lookups: &[&str], expected: &[&str]) {
		let mut builder = StringTableBuilder::new(0);
		for s in lookups {
			let _ = builder.lookup(s);
		}

		let actual = builder
			.map
			.into_values()
			.flat_map(|x| x)
			.map(|x| super::read_string(&builder.data, x).unwrap().to_string())
			.sorted()
			.collect::<Vec<_>>();
		assert_eq!(expected, actual.as_slice());
	}
}
