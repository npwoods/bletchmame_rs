//! Logic for parsing "InfoDb" databases; our internal representation of --listml output

mod binary;
mod build;
mod entities;
mod smallstr;
mod strings;

use std::cmp::min;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::marker::PhantomData;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;

use binary_serde::BinarySerde;
use binary_serde::Endianness;

use crate::prefs::prefs_filename;
use crate::Error;
use crate::Result;

pub use self::binary::ChipType;
pub use self::entities::ChipsView;
pub use self::entities::Machine;
pub use self::entities::MachinesView;
pub use self::smallstr::SmallStrRef;

use self::build::calculate_sizes_hash;
use self::build::data_from_listxml_output;
use self::strings::read_string;
use self::strings::validate_string_table;

const MAGIC_HDR: &[u8; 8] = b"MAMEINFO";
const ENDIANNESS: Endianness = Endianness::Little;

pub struct InfoDb {
	data: Box<[u8]>,
	machines: RootView<binary::Machine>,
	chips: RootView<binary::Chip>,
	strings_offset: usize,

	#[allow(dead_code)]
	build_strindex: u32,
}

impl InfoDb {
	pub fn new(data: Box<[u8]>) -> Result<Self> {
		Self::new_internal(data, false)
	}

	fn new_internal(data: Box<[u8]>, skip_validations: bool) -> Result<Self> {
		// first get the header
		let hdr = decode_header(&data)?;

		// now walk the views
		let mut cursor = binary::Header::SERIALIZED_SIZE..data.len();
		let machines = next_root_view(&mut cursor, hdr.machine_count)?;
		let chips = next_root_view(&mut cursor, hdr.chips_count)?;

		// validations we want to skip if we're creating things ourselves
		if !skip_validations {
			validate_string_table(&data[cursor.start..]).map_err(|_| Error::CorruptStringTable)?;
		}

		// and return
		let result = Self {
			data,
			machines,
			chips,
			strings_offset: cursor.start,
			build_strindex: hdr.build_strindex,
		};
		Ok(result)
	}

	pub fn load(mame_executable_path: &str) -> Result<Self> {
		let filename = infodb_filename(mame_executable_path).map_err(infodb_load_error)?;
		let file = File::open(filename).map_err(infodb_load_error)?;
		let mut reader = BufReader::new(file);
		let mut data = Vec::new();
		reader.read_to_end(&mut data).map_err(infodb_load_error)?;
		Self::new(data.into())
	}

	pub fn save(&self, mame_executable_path: &str) -> Result<()> {
		let filename = infodb_filename(mame_executable_path).map_err(infodb_load_error)?;
		let mut file = File::create(filename).map_err(infodb_save_error)?;
		file.write_all(&self.data).map_err(infodb_save_error)?;
		Ok(())
	}

	pub fn from_listxml_output(
		reader: impl BufRead,
		callback: impl FnMut(&str) -> bool,
	) -> crate::Result<Option<Self>> {
		// process 'mame -listxml' output
		let data = data_from_listxml_output(reader, callback)?;

		// bail if we cancelled
		let Some(data) = data else {
			return Ok(None);
		};

		// we've succeeded at this point (or else we did something absurdly wrong)
		let info_db = Self::new_internal(data, true).expect("data_from_listxml_output() created an invalid InfoDB");
		Ok(Some(info_db))
	}

	#[allow(dead_code)]
	pub fn build(&self) -> SmallStrRef<'_> {
		self.string(self.build_strindex)
	}

	pub fn machines(&self) -> MachinesView<'_> {
		self.make_view(&self.machines)
	}

	pub fn chips(&self) -> ChipsView<'_> {
		self.make_view(&self.chips)
	}

	fn string(&self, offset: u32) -> SmallStrRef<'_> {
		read_string(&self.data[self.strings_offset..], offset).unwrap_or_default()
	}

	fn make_view<B>(&self, root_view: &RootView<B>) -> View<'_, B>
	where
		B: BinarySerde,
	{
		let offset = root_view.offset.try_into().unwrap();
		let count = root_view.count.try_into().unwrap();
		View {
			db: self,
			offset,
			count,
			phantom: PhantomData,
		}
	}
}

impl Debug for InfoDb {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
		write!(f, "{:?}", self.data)
	}
}

fn next_root_view<T>(cursor: &mut Range<usize>, count: u32) -> Result<RootView<T>>
where
	T: BinarySerde,
{
	// get the result
	let offset = cursor
		.start
		.try_into()
		.map_err(|_| Error::CannotDeserializeInfoDbHeader)?;

	// advance the cursor
	let count_bytes = count
		.checked_mul(T::SERIALIZED_SIZE.try_into().unwrap())
		.ok_or(Error::CannotDeserializeInfoDbHeader)?;
	let new_start = cursor
		.start
		.checked_add(count_bytes.try_into().unwrap())
		.ok_or(Error::CannotDeserializeInfoDbHeader)?;
	if new_start > cursor.end {
		return Err(Error::CannotDeserializeInfoDbHeader.into());
	}
	*cursor = new_start..(cursor.end);

	// and return
	let phantom = PhantomData;
	Ok(RootView { offset, count, phantom })
}

#[derive(Clone, Copy, Debug)]
struct RootView<T> {
	offset: u32,
	count: u32,
	phantom: PhantomData<T>,
}

fn infodb_filename(mame_executable_path: &str) -> Result<PathBuf> {
	let file_name = Path::new(mame_executable_path)
		.file_name()
		.ok_or(Error::CannotBuildInfoDbFilename)?;
	let file_stem = Path::new(file_name)
		.file_stem()
		.ok_or(Error::CannotBuildInfoDbFilename)?;
	let file_name = Path::new(file_stem).with_extension("infodb");
	prefs_filename(Some(&file_name.as_path().to_string_lossy()))
}

fn infodb_load_error(e: impl std::error::Error + Send + Sync + 'static) -> Error {
	Error::PreferencesLoad(e.into())
}

fn infodb_save_error(e: impl std::error::Error + Send + Sync + 'static) -> Error {
	Error::PreferencesSave(e.into())
}

fn decode_header(data: &[u8]) -> Result<binary::Header> {
	let header_data = &data[0..min(binary::Header::SERIALIZED_SIZE, data.len())];
	let header = binary::Header::binary_deserialize(header_data, ENDIANNESS)
		.map_err(|_| Error::CannotDeserializeInfoDbHeader)?;
	if &header.magic != MAGIC_HDR {
		return Err(Box::new(Error::BadInfoDbMagicValue));
	}
	if header.sizes_hash != calculate_sizes_hash() {
		return Err(Box::new(Error::BadInfoDbSizesHash));
	}
	Ok(header)
}

#[derive(Clone, Copy)]
pub struct View<'a, B>
where
	B: BinarySerde,
{
	db: &'a InfoDb,
	offset: usize,
	count: usize,
	phantom: PhantomData<B>,
}

impl<'a, B> View<'a, B>
where
	B: BinarySerde,
{
	pub fn iter(&self) -> impl Iterator<Item = Object<'a, B>> {
		ViewIter {
			view: View {
				db: self.db,
				offset: self.offset,
				count: self.count,
				phantom: PhantomData,
			},
			pos: 0,
		}
	}

	pub fn len(&self) -> usize {
		self.count
	}

	pub fn get(&self, index: usize) -> Option<Object<'a, B>> {
		(index < self.count).then(|| Object {
			db: self.db,
			offset: self.offset + index * B::SERIALIZED_SIZE,
			phantom: PhantomData,
		})
	}

	pub fn sub_view(&self, index: u32, count: u32) -> View<'a, B> {
		let index = usize::try_from(index).unwrap();
		let count = usize::try_from(count).unwrap();
		let offset = self.offset + index * B::SERIALIZED_SIZE;
		assert!(offset <= self.db.data.len());
		assert!(offset + count * B::SERIALIZED_SIZE <= self.db.data.len());
		View {
			db: self.db,
			offset,
			count,
			phantom: PhantomData,
		}
	}
}

#[derive(Clone, Copy)]
struct ViewIter<'a, B>
where
	B: BinarySerde,
{
	view: View<'a, B>,
	pos: usize,
}

impl<'a, B> Iterator for ViewIter<'a, B>
where
	B: BinarySerde,
{
	type Item = Object<'a, B>;

	fn next(&mut self) -> Option<Self::Item> {
		let result = (self.view).get(self.pos);
		if result.is_some() {
			self.pos += 1;
		}
		result
	}
}

#[derive(Clone, Copy)]
pub struct Object<'a, B>
where
	B: BinarySerde,
{
	db: &'a InfoDb,
	offset: usize,
	phantom: PhantomData<B>,
}

impl<'a, B> Object<'a, B>
where
	B: BinarySerde,
{
	fn obj(&self) -> B {
		let data = &self.db.data[self.offset..(self.offset + B::SERIALIZED_SIZE)];
		B::binary_deserialize(data, ENDIANNESS).unwrap()
	}

	fn string(&self, func: impl FnOnce(B) -> u32) -> SmallStrRef<'a> {
		let offset = func(self.obj());
		self.db.string(offset)
	}
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::{ChipType, InfoDb};

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), "0.229 (mame0229)", 13, 1, &[("alienar", "1985"),("ipt_merge_any_hi", ""),("ls157", "")])]
	#[test_case(1, include_str!("test_data/listxml_coco.xml"), "0.229 (mame0229)", 104, 15, &[("acia6850", ""), ("address_map_bank", ""), ("ay8910", "")])]
	#[test_case(2, include_str!("test_data/listxml_fake.xml"), "<<fake build>>", 2, 1, &[("fake", "2021"),("mc6809e", "")])]
	pub fn test(
		_index: usize,
		xml: &str,
		expected_build: &str,
		expected_machines_count: usize,
		expected_runnable_machine_count: usize,
		initial_expected: &[(&str, &str)],
	) {
		let initial_expected = initial_expected
			.iter()
			.map(|(name, year)| (name.to_string(), year.to_string()))
			.collect::<Vec<_>>();
		let expected = (
			expected_build.to_string(),
			expected_machines_count,
			expected_runnable_machine_count,
			initial_expected.as_slice(),
		);

		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| false).unwrap().unwrap();
		let actual_initial_machines = db
			.machines()
			.iter()
			.take(initial_expected.len())
			.map(|m| (m.name().to_string(), m.year().to_string()))
			.collect::<Vec<_>>();
		let actual_runnable_machine_count = db.machines().iter().filter(|m| m.runnable()).count();
		let actual = (
			db.build().to_string(),
			db.machines().len(),
			actual_runnable_machine_count,
			actual_initial_machines.as_slice(),
		);
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), 0, Some(("alienar", "1985")))]
	#[test_case(1, include_str!("test_data/listxml_alienar.xml"), 5, Some(("mc6809e", "")))]
	#[test_case(2, include_str!("test_data/listxml_alienar.xml"), 4242, None)]
	pub fn machines_get(_index: usize, xml: &str, index: usize, expected: Option<(&str, &str)>) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| false).unwrap().unwrap();
		let actual = db
			.machines()
			.get(index)
			.map(|x| (String::from(x.name()), String::from(x.year())));

		let expected = expected.map(|(name, year)| (name.to_string(), year.to_string()));
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), "alienar", Some(("Duncan Brown", "1985")))]
	#[test_case(1, include_str!("test_data/listxml_coco.xml"), "cocoe", Some(("Tandy Radio Shack", "1981")))]
	#[test_case(2, include_str!("test_data/listxml_coco.xml"), "coco2b", Some(("Tandy Radio Shack", "1985?")))]
	#[test_case(3, include_str!("test_data/listxml_fake.xml"), "fake", Some(("<Bletch>", "2021")))]
	#[test_case(4, include_str!("test_data/listxml_fake.xml"), "NONEXISTANT", None)]
	pub fn machines_find(_index: usize, xml: &str, target: &str, expected: Option<(&str, &str)>) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| false).unwrap().unwrap();
		let actual = db
			.machines()
			.find(target)
			.map(|x| (String::from(x.manufacturer()), String::from(x.year())));

		let expected = expected.map(|(manufacturer, year)| (manufacturer.to_string(), year.to_string()));
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"))]
	pub fn machines_find_everything(_index: usize, xml: &str) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| false).unwrap().unwrap();
		for machine in db.machines().iter() {
			let other_machine = db.machines().find(&machine.name());
			assert_eq!(other_machine.map(|m| m.name()), Some(machine.name()));
		}
	}

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), "alienar", &[(ChipType::Cpu, "maincpu"), (ChipType::Cpu, "soundcpu"), (ChipType::Audio, "speaker"), (ChipType::Audio, "dac")])]
	#[test_case(1, include_str!("test_data/listxml_fake.xml"), "fake", &[(ChipType::Cpu, "maincpu")])]
	pub fn chips(_index: usize, xml: &str, machine: &str, expected: &[(ChipType, &str)]) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| false).unwrap().unwrap();
		let actual = db
			.machines()
			.find(machine)
			.unwrap()
			.chips()
			.iter()
			.map(|chip| (chip.chip_type(), chip.tag().to_string()))
			.collect::<Vec<_>>();

		let expected = expected
			.into_iter()
			.map(|(chip_type, tag)| (*chip_type, tag.to_string()))
			.collect::<Vec<_>>();
		assert_eq!(expected, actual);
	}
}
