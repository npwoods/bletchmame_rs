use std::collections::HashMap;
use std::hash::DefaultHasher;
use std::hash::Hasher;

use anyhow::Error;
use anyhow::Result;
use smallvec::SmallVec;

use crate::info::UsizeDb;

const MAGIC_STRINGTABLE_BEGIN: &[u8; 2] = &[0x9D, 0x9B];
const MAGIC_STRINGTABLE_END: &[u8; 2] = &[0x9F, 0x99];

#[derive(Debug)]
pub struct StringTableBuilder {
	data: Vec<u8>,
	map: HashMap<u64, SmallVec<[UsizeDb; 4]>>,
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
		result.map_insert("", UsizeDb::default());

		// and return
		result
	}

	pub fn lookup(&mut self, s: &str) -> UsizeDb {
		self.lookup_immut(s).unwrap_or_else(|| {
			if !self.data.len() > MAGIC_STRINGTABLE_BEGIN.len() {
				self.data.push(0x80);
			}
			let result = self.data.len();
			self.data.extend(s.as_bytes());

			for (pos, _) in s.char_indices() {
				let element = result + pos;
				self.map_insert(&s[pos..], element.try_into().unwrap());
			}
			result.try_into().unwrap()
		})
	}

	pub fn index(&self, offset: impl Into<usize>) -> &'_ str {
		read_string(&self.data, offset).unwrap()
	}

	pub fn into_iter(mut self) -> impl Iterator<Item = u8> {
		self.data.extend(MAGIC_STRINGTABLE_END.iter());
		self.data.into_iter()
	}

	pub fn lookup_immut(&self, s: &str) -> Option<UsizeDb> {
		self.map
			.get(&hash(s))?
			.iter()
			.copied()
			.find(|&offset| s == self.index(offset))
	}

	fn map_insert(&mut self, s: &str, element: UsizeDb) {
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

pub fn read_string(data: &[u8], offset: impl Into<usize>) -> Result<&'_ str> {
	let offset = offset.into();
	let data = data.get(offset..).ok_or_else(|| {
		let message = format!("read_string(): Invalid offset {offset}");
		Error::msg(message)
	})?;
	let result = data.utf8_chunks().next().map(|x| x.valid()).unwrap_or_default();
	Ok(result)
}

pub fn validate_string_table(data: &[u8]) -> Result<()> {
	if data.get(..MAGIC_STRINGTABLE_BEGIN.len()) != Some(MAGIC_STRINGTABLE_BEGIN) {
		let message = "Invalid magic bytes at beginning of string table";
		return Err(Error::msg(message));
	}
	if data.get((data.len() - MAGIC_STRINGTABLE_END.len())..) != Some(MAGIC_STRINGTABLE_END) {
		let message = "Invalid magic bytes at end of string table";
		return Err(Error::msg(message));
	}

	let middle_data = &data[(MAGIC_STRINGTABLE_BEGIN.len())..(data.len() - MAGIC_STRINGTABLE_END.len())];
	middle_data
		.utf8_chunks()
		.all(|chunk| chunk.invalid().is_empty() || chunk.invalid() == [0x80])
		.then_some(())
		.ok_or_else(|| {
			let message = "Corrupt data within string table";
			Error::msg(message)
		})
}

fn hash(s: &str) -> u64 {
	let mut hasher = DefaultHasher::new();
	hasher.write(s.as_bytes());
	hasher.finish()
}

#[cfg(test)]
mod test {
	use itertools::Itertools;
	use test_case::test_case;

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
		let actual = usize::from(builder.lookup(""));
		assert_eq!(0, actual);
	}

	#[test_case(0, 0, Ok(""), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(1, 2, Ok("foo"), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(2, 6, Ok("bar"), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(3, 10, Ok(""), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(4, 11, Ok(""), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	#[test_case(5, 4242, Err(()), b"\x9D\x9Bfoo\x80bar\x9F\x99")]
	pub fn read_string(_index: usize, offset: usize, expected: std::result::Result<&str, ()>, bytes: &[u8]) {
		let expected = expected.map(String::from);
		let actual = super::read_string(bytes, offset);
		let actual = actual.map(String::from).map_err(|_| ());
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
			assert_eq!((*other, len), (result, builder.data.len()));
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
			.flatten()
			.map(|x| super::read_string(&builder.data, x).unwrap().to_string())
			.sorted()
			.collect::<Vec<_>>();
		assert_eq!(expected, actual.as_slice());
	}
}
