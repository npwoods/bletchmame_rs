//! Logic for parsing "InfoDb" databases; our internal representation of --listml output
mod binary;
mod build;
mod entities;
mod strings;

use std::any::type_name;
use std::cmp::Ordering;
use std::cmp::min;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Cursor;
use std::io::Read;
use std::io::Write;
use std::marker::PhantomData;
use std::ops::AddAssign;
use std::ops::ControlFlow;
use std::ops::Not;
use std::ops::Range;
use std::ops::Sub;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use anyhow::Error;
use anyhow::Result;
use anyhow::ensure;
use byteorder::LittleEndian;
use byteorder::ReadBytesExt;
use byteorder::WriteBytesExt;
use entities::SoftwareListsView;
use more_asserts::assert_ge;
use more_asserts::assert_le;
use zerocopy::Immutable;
use zerocopy::IntoBytes;
use zerocopy::KnownLayout;
use zerocopy::TryFromBytes;
use zerocopy::little_endian::U16;

use crate::platform::CommandExt;
use crate::prefs::prefs_filename;
use crate::version::MameVersion;

pub use self::binary::ChipType;
pub use self::binary::ConditionRelation;
pub use self::binary::SoftwareListStatus;
pub use self::entities::BiosSet;
pub use self::entities::Chip;
pub use self::entities::Configuration;
pub use self::entities::ConfigurationSetting;
pub use self::entities::ConfigurationSettingCondition;
pub use self::entities::Device;
pub use self::entities::Disk;
pub use self::entities::Machine;
pub use self::entities::MachineSoftwareList;
pub use self::entities::MachinesView;
pub use self::entities::RamOption;
pub use self::entities::Rom;
pub use self::entities::Sample;
pub use self::entities::Slot;
pub use self::entities::SlotOption;
pub use self::entities::SoftwareList;

use self::build::calculate_sizes_hash;
use self::build::data_from_listxml_output;
use self::strings::read_string;
use self::strings::validate_string_table;

const MAGIC_HDR: &[u8; 8] = b"MAMEINFO";
const SERIAL: u16 = 1;

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("InfoDb is corrupted")]
	Validation(Vec<Error>),
	#[error("Cannot deserialize InfoDb header")]
	CannotDeserializeHeader,
}

pub struct InfoDb {
	data: Box<[u8]>,
	machines: RootView<binary::Machine>,
	roms: RootView<binary::Rom>,
	disks: RootView<binary::Disk>,
	samples: RootView<binary::Sample>,
	biossets: RootView<binary::BiosSet>,
	chips: RootView<binary::Chip>,
	configs: RootView<binary::Configuration>,
	config_settings: RootView<binary::ConfigurationSetting>,
	config_setting_conditions: RootView<binary::ConfigurationSettingCondition>,
	devices: RootView<binary::Device>,
	slots: RootView<binary::Slot>,
	slot_options: RootView<binary::SlotOption>,
	software_lists: RootView<binary::SoftwareList>,
	software_list_machine_indexes: RootView<UsizeDb>,
	machine_software_lists: RootView<binary::MachineSoftwareList>,
	ram_options: RootView<binary::RamOption>,
	strings_offset: usize,
	build: MameVersion,
}

impl InfoDb {
	pub fn new(data: Box<[u8]>) -> Result<Self> {
		Self::new_internal(data, false)
	}

	fn new_internal(data: Box<[u8]>, skip_validations: bool) -> Result<Self> {
		// first get the header
		let hdr = decode_header(&data)?;

		// now walk the views
		let mut cursor = size_of::<binary::Header>()..data.len();
		let machines = next_root_view(&mut cursor, hdr.machine_count)?;
		let roms = next_root_view(&mut cursor, hdr.rom_count)?;
		let disks = next_root_view(&mut cursor, hdr.disk_count)?;
		let samples = next_root_view(&mut cursor, hdr.sample_count)?;
		let biossets = next_root_view(&mut cursor, hdr.biosset_count)?;
		let chips = next_root_view(&mut cursor, hdr.chips_count)?;
		let configs = next_root_view(&mut cursor, hdr.config_count)?;
		let config_settings = next_root_view(&mut cursor, hdr.config_setting_count)?;
		let config_setting_conditions = next_root_view(&mut cursor, hdr.config_setting_condition_count)?;
		let devices = next_root_view(&mut cursor, hdr.device_count)?;
		let slots = next_root_view(&mut cursor, hdr.slot_count)?;
		let slot_options = next_root_view(&mut cursor, hdr.slot_option_count)?;
		let software_lists = next_root_view(&mut cursor, hdr.software_list_count)?;
		let software_list_machine_indexes = next_root_view(&mut cursor, hdr.software_list_machine_count)?;
		let machine_software_lists = next_root_view(&mut cursor, hdr.machine_software_lists_count)?;
		let ram_options = next_root_view(&mut cursor, hdr.ram_option_count)?;

		// get the build
		let build_str = read_string(&data[cursor.start..], hdr.build_strindex).unwrap_or_default();
		let build = MameVersion::from(build_str);

		// and return
		let result = Self {
			data,
			machines,
			roms,
			disks,
			samples,
			biossets,
			chips,
			configs,
			config_settings,
			config_setting_conditions,
			devices,
			slots,
			slot_options,
			software_lists,
			software_list_machine_indexes,
			machine_software_lists,
			ram_options,
			strings_offset: cursor.start,
			build,
		};

		// more validations
		if !skip_validations {
			result.validate().map_err(ThisError::Validation)?;
		}

		Ok(result)
	}

	pub fn load(prefs_path: impl AsRef<Path>, mame_executable_path: &str) -> Result<Self> {
		let filename = infodb_filename(prefs_path, mame_executable_path).map_err(infodb_load_error)?;
		let file = File::open(filename).map_err(infodb_load_error)?;
		let mut reader = BufReader::new(file);
		let mut data = Vec::new();
		reader.read_to_end(&mut data).map_err(infodb_load_error)?;
		Self::new(data.into())
	}

	pub fn save(&self, prefs_path: impl AsRef<Path>, mame_executable_path: &str) -> Result<()> {
		let filename = infodb_filename(prefs_path, mame_executable_path).map_err(infodb_save_error)?;
		let mut file = File::create(filename).map_err(infodb_save_error)?;
		file.write_all(&self.data).map_err(infodb_save_error)?;
		Ok(())
	}

	pub fn from_listxml_output(
		reader: impl BufRead,
		callback: impl FnMut(&str) -> ControlFlow<()>,
	) -> Result<Option<Self>> {
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

	pub fn from_child_process(
		mame_executable_path: &str,
		callback: impl FnMut(&str) -> ControlFlow<()>,
	) -> Result<Option<Self>> {
		// launch the process
		let mut process = Command::new(mame_executable_path)
			.arg("-listxml")
			.arg("-nodtd")
			.stdout(Stdio::piped())
			.create_no_window(true)
			.spawn()?;

		// access the MAME process stdout (which is input to us)
		let input = process.stdout.as_mut().unwrap();

		// process the InfoDB output
		let reader = BufReader::new(input);
		let db = InfoDb::from_listxml_output(reader, callback);

		// if we either cancelled or errored, try to kill the process
		if !matches!(db, Ok(Some(_))) {
			let _ = process.kill();
		}

		// and close out the process (we don't want it to zombie)
		let _ = process.wait();

		// and return!
		db
	}

	pub fn validate(&self) -> std::result::Result<(), Vec<Error>> {
		// prepare a vec of errors
		let mut errors = Vec::new();
		let mut emit = |e| errors.push(e);

		// validate the views
		validate_view(self.machine_software_lists(), &mut emit);
		validate_view_custom(
			self.software_list_machine_indexes(),
			&mut emit,
			"SoftwareListMachineIndex",
			|x| {
				ensure!(usize::from(*x.obj()) < self.machines().len());
				Ok(())
			},
		);

		// validate the string table
		if let Err(e) = validate_string_table(&self.data[self.strings_offset..]) {
			let message = format!("Corrupt string table: {e}");
			errors.push(Error::msg(message));
		}

		// ..and finish up
		if errors.is_empty() { Ok(()) } else { Err(errors) }
	}

	pub fn data_len(&self) -> usize {
		self.data.len()
	}

	pub fn strings_len(&self) -> usize {
		self.data.len() - self.strings_offset
	}

	pub fn build(&self) -> &MameVersion {
		&self.build
	}

	pub fn machines(&self) -> MachinesView<'_> {
		self.make_view(&self.machines)
	}

	pub fn roms(&self) -> impl View<'_, Rom<'_>> {
		self.make_view(&self.roms)
	}

	pub fn disks(&self) -> impl View<'_, Disk<'_>> {
		self.make_view(&self.disks)
	}

	pub fn samples(&self) -> impl View<'_, Sample<'_>> {
		self.make_view(&self.samples)
	}

	pub fn biossets(&self) -> impl View<'_, BiosSet<'_>> {
		self.make_view(&self.biossets)
	}

	pub fn chips(&self) -> impl View<'_, Chip<'_>> {
		self.make_view(&self.chips)
	}

	pub fn configurations(&self) -> impl View<'_, Configuration<'_>> {
		self.make_view(&self.configs)
	}

	pub fn configuration_settings(&self) -> impl View<'_, ConfigurationSetting<'_>> {
		self.make_view(&self.config_settings)
	}

	pub fn configuration_setting_conditions(&self) -> impl View<'_, ConfigurationSettingCondition<'_>> {
		self.make_view(&self.config_setting_conditions)
	}

	pub fn devices(&self) -> impl View<'_, Device<'_>> {
		self.make_view(&self.devices)
	}

	pub fn slots(&self) -> impl View<'_, Slot<'_>> {
		self.make_view(&self.slots)
	}

	pub fn slot_options(&self) -> impl View<'_, SlotOption<'_>> {
		self.make_view(&self.slot_options)
	}

	pub fn software_lists(&self) -> SoftwareListsView<'_> {
		self.make_view(&self.software_lists)
	}

	pub fn machine_software_lists(&self) -> impl View<'_, MachineSoftwareList<'_>> {
		self.make_view(&self.machine_software_lists)
	}

	pub fn software_list_machine_indexes(&self) -> impl View<'_, Object<'_, UsizeDb>> {
		self.make_view(&self.software_list_machine_indexes)
	}

	pub fn ram_options(&self) -> impl View<'_, RamOption<'_>> {
		self.make_view(&self.ram_options)
	}

	fn string(&self, offset: UsizeDb) -> &'_ str {
		read_string(&self.data[self.strings_offset..], offset).unwrap_or_default()
	}

	fn make_view<B>(&self, root_view: &RootView<B>) -> SimpleView<'_, B> {
		SimpleView {
			db: Some(self),
			byte_offset: root_view.offset,
			start: 0,
			end: root_view.count,
			phantom: PhantomData,
		}
	}
}

impl Debug for InfoDb {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
		f.debug_struct("InfoDb")
			.field("data.len()", &self.data.len())
			.finish_non_exhaustive()
	}
}

fn next_root_view<T>(cursor: &mut Range<usize>, count: UsizeDb) -> Result<RootView<T>> {
	let error_message = "Cannot deserialize InfoDB header";

	// get the result
	let offset = cursor.start;

	// advance the cursor
	let count = usize::from(count);
	let count_bytes = count
		.checked_mul(size_of::<T>())
		.ok_or_else(|| Error::msg(error_message))?;
	let new_start = cursor
		.start
		.checked_add(count_bytes)
		.ok_or_else(|| Error::msg(error_message))?;
	if new_start > cursor.end {
		return Err(Error::msg(error_message));
	}
	*cursor = new_start..(cursor.end);

	// and return
	let phantom = PhantomData;
	Ok(RootView { offset, count, phantom })
}

#[derive(Clone, Copy, Debug)]
struct RootView<T> {
	offset: usize,
	count: usize,
	phantom: PhantomData<T>,
}

fn infodb_filename(prefs_path: impl AsRef<Path>, mame_executable_path: &str) -> Result<PathBuf> {
	let file_name = Path::new(mame_executable_path)
		.file_name()
		.ok_or_else(infodb_filename_error)?;
	let file_stem = Path::new(file_name).file_stem().ok_or_else(infodb_filename_error)?;
	let file_name = Path::new(file_stem).with_extension("infodb");
	prefs_filename(prefs_path, Some(&file_name.as_path().to_string_lossy()))
}

fn infodb_load_error(error: impl Into<Error>) -> Error {
	error.into().context("Error loading InfoDB")
}

fn infodb_save_error(error: impl Into<Error>) -> Error {
	error.into().context("Error saving InfoDB")
}

fn infodb_filename_error() -> Error {
	Error::msg("Cannot determine InfoDB filename")
}

fn decode_header(data: &[u8]) -> Result<&binary::Header> {
	let header_data = &data[0..min(size_of::<binary::Header>(), data.len())];
	let header = binary::Header::try_ref_from_bytes(header_data)
		.ok()
		.ok_or(ThisError::CannotDeserializeHeader)?;
	if header.magic != *MAGIC_HDR {
		return Err(Error::msg("Bad InfoDB Magic Value In Header"));
	}
	if header.serial != U16::from(SERIAL) {
		return Err(Error::msg("Bad InfoDB Serial Value In Header"));
	}
	if header.sizes_hash != calculate_sizes_hash() {
		return Err(Error::msg("Bad Sizes Hash In Header"));
	}
	Ok(header)
}

pub trait View<'a, T>: Clone
where
	T: 'a,
	Self: 'a,
{
	fn get(&self, index: usize) -> Option<T>;
	fn len(&self) -> usize;
	fn sub_view(&self, range: Range<usize>) -> Self;

	fn iter(&self) -> impl Iterator<Item = T> + 'a {
		ViewIter {
			view: self.clone(),
			pos: 0,
			phantom: PhantomData,
		}
	}

	fn is_empty(&self) -> bool {
		self.len() == 0
	}
}

#[derive(Clone, Copy)]
struct ViewIter<V, T> {
	view: V,
	pos: usize,
	phantom: PhantomData<T>,
}

impl<'a, V, T> Iterator for ViewIter<V, T>
where
	V: View<'a, T>,
	T: 'a,
{
	type Item = T;

	fn next(&mut self) -> Option<Self::Item> {
		let result = (self.view).get(self.pos);
		if result.is_some() {
			self.pos += 1;
		}
		result
	}
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SimpleView<'a, B> {
	db: Option<&'a InfoDb>,
	byte_offset: usize,
	start: usize,
	end: usize,
	phantom: PhantomData<&'a B>,
}

impl<'a, B> View<'a, Object<'a, B>> for SimpleView<'a, B>
where
	B: TryFromBytes + Clone,
{
	fn len(&self) -> usize {
		self.end - self.start
	}

	fn get(&self, index: usize) -> Option<Object<'a, B>> {
		(index < self.len()).then(|| Object {
			db: self.db.expect("expected InfoDb to be Some"),
			byte_offset: self.byte_offset,
			index: self.start + index,
			phantom: PhantomData,
		})
	}

	fn sub_view(&self, range: Range<usize>) -> Self {
		assert_le!(range.start, range.end);
		assert_le!(range.start, self.end - self.start);
		assert_le!(range.end, self.end - self.start);

		Self {
			start: self.start + range.start,
			end: self.start + range.end,
			..*self
		}
	}
}

#[derive(Clone, Copy, Debug)]
pub struct IndirectView<V, W> {
	index_view: V,
	object_view: W,
}

impl<'a, B, VI, VO> View<'a, Object<'a, B>> for IndirectView<VI, VO>
where
	B: TryFromBytes + Clone + 'a,
	VI: View<'a, Object<'a, UsizeDb>> + 'a,
	VO: View<'a, Object<'a, B>> + 'a,
{
	fn len(&self) -> usize {
		self.index_view.len()
	}

	fn get(&self, index: usize) -> Option<Object<'a, B>> {
		let object_index = usize::from(*self.index_view.get(index)?.obj());
		let obj = self
			.object_view
			.get(object_index)
			.expect("IndirectView::get(): object_index out of range");
		Some(obj)
	}

	fn sub_view(&self, range: Range<usize>) -> Self {
		let index_view = self.index_view.sub_view(range);
		let object_view = self.object_view.clone();
		Self {
			index_view,
			object_view,
		}
	}
}

#[derive(Clone, Copy)]
pub struct Object<'a, B> {
	db: &'a InfoDb,
	byte_offset: usize,
	index: usize,
	phantom: PhantomData<B>,
}

impl<B> Object<'_, B> {
	pub fn index(&self) -> usize {
		self.index
	}

	fn proxy(&self) -> impl PartialEq {
		(self.db as *const _, self.byte_offset, self.index)
	}
}

impl<'a, B> Object<'a, B>
where
	B: TryFromBytes + KnownLayout + Immutable,
{
	fn obj(&self) -> &'_ B {
		let start = self.byte_offset + self.index * size_of::<B>();
		let end = start + size_of::<B>();
		let buf = &self.db.data[start..end];
		TryFromBytes::try_ref_from_bytes(buf).unwrap()
	}

	fn string(&self, func: impl FnOnce(&B) -> UsizeDb) -> &'a str {
		let offset = func(self.obj());
		self.db.string(offset)
	}
}

impl<B> PartialEq for Object<'_, B> {
	fn eq(&self, other: &Self) -> bool {
		self.proxy() == other.proxy()
	}
}

impl<B> Debug for Object<'_, B>
where
	B: TryFromBytes + KnownLayout + Immutable + Debug,
{
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Object")
			.field("byte_offset", &self.byte_offset)
			.field("index", &self.index)
			.field("obj", &self.obj())
			.finish()
	}
}

trait Validatable {
	fn validate(&self) -> Result<()>;
}

fn validate_view<'a, T>(view: impl View<'a, Object<'a, T>>, emit: &mut impl FnMut(Error))
where
	T: 'a,
	Object<'a, T>: Validatable,
{
	let type_name = type_name::<T>();
	let type_name = type_name.rsplit_once("::").map(|x| x.1).unwrap_or(type_name);
	validate_view_custom(view, emit, type_name, |obj| obj.validate());
}

fn validate_view_custom<'a, T>(
	view: impl View<'a, Object<'a, T>>,
	emit: &mut impl FnMut(Error),
	type_name: &str,
	validate_func: impl Fn(Object<'a, T>) -> Result<()>,
) where
	T: 'a,
{
	for (index, obj) in view.iter().enumerate() {
		if let Err(e) = validate_func(obj) {
			let message = format!("{type_name}[{index}]: {e}");
			emit(Error::msg(message));
		}
	}
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default, Hash, PartialEq, Eq, TryFromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct UsizeDb([u8; 3]);

impl AddAssign<usize> for UsizeDb {
	fn add_assign(&mut self, rhs: usize) {
		let result = usize::from(*self) + rhs;
		self.0 = Self::try_from(result).unwrap().0;
	}
}

impl Sub for UsizeDb {
	type Output = Self;

	fn sub(self, rhs: Self) -> Self::Output {
		assert_ge!(self, rhs);
		(usize::from(self) - usize::from(rhs)).try_into().unwrap()
	}
}

impl Not for UsizeDb {
	type Output = Self;

	fn not(self) -> Self::Output {
		Self([!self.0[0], !self.0[1], !self.0[2]])
	}
}

impl Ord for UsizeDb {
	fn cmp(&self, other: &Self) -> Ordering {
		Ord::cmp(&usize::from(*self), &usize::from(*other))
	}
}

impl PartialOrd for UsizeDb {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(Ord::cmp(self, other))
	}
}

impl TryFrom<usize> for UsizeDb {
	type Error = Error;

	fn try_from(value: usize) -> Result<Self> {
		const ERROR_MESSAGE: &str = "usize too large";

		let value = u32::try_from(value).map_err(|_| Error::msg(ERROR_MESSAGE))?;
		let mut bytes = [0_u8, 0_u8, 0_u8];
		let mut cursor = Cursor::new(bytes.as_mut_slice());
		cursor
			.write_u24::<LittleEndian>(value)
			.map_err(|_| Error::msg(ERROR_MESSAGE))?;
		Ok(UsizeDb(bytes))
	}
}

impl From<UsizeDb> for usize {
	fn from(value: UsizeDb) -> Self {
		const ERROR_MESSAGE: &str = "unexpected error converting UsizeDb to usize";

		let mut cursor = Cursor::new(&value.0);
		let value = cursor.read_u24::<LittleEndian>().expect(ERROR_MESSAGE);
		usize::try_from(value).expect(ERROR_MESSAGE)
	}
}

impl Debug for UsizeDb {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		usize::from(*self).fmt(f)
	}
}

#[cfg(test)]
mod test {
	use std::cmp::max;
	use std::ops::ControlFlow;

	use itertools::Itertools;
	use test_case::test_case;

	use crate::assethash::AssetHash;

	use super::ChipType;
	use super::InfoDb;
	use super::View;

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), "0.229 (mame0229)", 13, 1, &["alienar", "ipt_merge_any_hi", "ls157"])]
	#[test_case(1, include_str!("test_data/listxml_coco.xml"), "0.273 (mame0273)", 121, 10, &["acia6850", "address_map_bank", "ata_interface"])]
	#[test_case(2, include_str!("test_data/listxml_fake.xml"), "<<fake build>>", 4, 3, &["blah", "fake", "fakefake", "mc6809e"])]
	pub fn test(
		_index: usize,
		xml: &str,
		expected_build: &str,
		expected_machines_count: usize,
		expected_runnable_machine_count: usize,
		initial_expected: &[&str],
	) {
		let initial_expected = initial_expected.iter().map(|name| name.to_string()).collect::<Vec<_>>();
		let expected = (
			expected_build.to_string(),
			expected_machines_count,
			expected_runnable_machine_count,
			initial_expected.as_slice(),
		);

		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let actual_initial_machines = db
			.machines()
			.iter()
			.take(initial_expected.len())
			.map(|m| m.name().to_string())
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

	#[allow(clippy::too_many_arguments)]
	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), "alienar", "Alien Arena", "1985", "Duncan Brown", "williams.cpp", None, None)]
	#[test_case(1, include_str!("test_data/listxml_c64.xml"), "c64", "Commodore 64 (NTSC)", "1982", "Commodore Business Machines", "commodore/c64.cpp", None, None)]
	#[test_case(2, include_str!("test_data/listxml_coco.xml"), "coco2b", "Color Computer 2B", "1985?", "Tandy Radio Shack", "trs/coco12.cpp", Some("coco"), Some("coco"))]
	#[test_case(3, include_str!("test_data/listxml_fake.xml"), "fake", "Fake Machine", "2021", "<Bletch>", "fake_machine.cpp", None, None)]
	pub fn machine(
		_index: usize,
		xml: &str,
		name: &str,
		expected_description: &str,
		expected_year: &str,
		expected_manufacturer: &str,
		expected_source_file: &str,
		expected_clone_of: Option<&str>,
		expected_rom_of: Option<&str>,
	) {
		let expected = (
			name.to_string(),
			expected_description.to_string(),
			expected_year.to_string(),
			expected_manufacturer.to_string(),
			expected_source_file.to_string(),
			expected_clone_of.map(|x| x.to_string()),
			expected_rom_of.map(|x| x.to_string()),
		);

		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let machine = db.machines().find(name).unwrap();
		let actual = (
			machine.name().to_string(),
			machine.description().to_string(),
			machine.year().to_string(),
			machine.manufacturer().to_string(),
			machine.source_file().to_string(),
			machine.clone_of().map(|x| x.name().to_string()),
			machine.rom_of().map(|x| x.name().to_string()),
		);

		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), 0, Some(("alienar", "1985")))]
	#[test_case(1, include_str!("test_data/listxml_alienar.xml"), 5, Some(("mc6809e", "")))]
	#[test_case(2, include_str!("test_data/listxml_alienar.xml"), 4242, None)]
	pub fn machines_get(_index: usize, xml: &str, index: usize, expected: Option<(&str, &str)>) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let actual = db
			.machines()
			.get(index)
			.map(|x| (String::from(x.name()), String::from(x.year())));

		let expected = expected.map(|(name, year)| (name.to_string(), year.to_string()));
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), "alienar", Some(("Duncan Brown", "1985")))]
	#[test_case(1, include_str!("test_data/listxml_coco.xml"), "coco", Some(("Tandy Radio Shack", "1980")))]
	#[test_case(2, include_str!("test_data/listxml_coco.xml"), "coco2b", Some(("Tandy Radio Shack", "1985?")))]
	#[test_case(3, include_str!("test_data/listxml_fake.xml"), "fake", Some(("<Bletch>", "2021")))]
	#[test_case(4, include_str!("test_data/listxml_fake.xml"), "NONEXISTANT", None)]
	pub fn machines_find(_index: usize, xml: &str, target: &str, expected: Option<(&str, &str)>) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let actual = db
			.machines()
			.find(target)
			.map(|x| (String::from(x.manufacturer()), String::from(x.year())))
			.ok();

		let expected = expected.map(|(manufacturer, year)| (manufacturer.to_string(), year.to_string()));
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"))]
	pub fn machines_find_everything(_index: usize, xml: &str) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		for machine in db.machines().iter() {
			let other_machine = db.machines().find(machine.name()).unwrap();
			assert_eq!(other_machine.name(), machine.name());
		}
	}

	#[test_case(0, include_str!("test_data/listxml_coco.xml"), "coco2b", "bas13.rom", "d8f4d15e", "28b92bebe35fa4f026a084416d6ea3b1552b63d3")]
	pub fn roms(_index: usize, xml: &str, machine: &str, rom: &str, expected_crc: &str, expected_sha1: &str) {
		let expected = AssetHash::from_hex_strings(Some(expected_crc), Some(expected_sha1)).unwrap();

		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let machine = db.machines().find(machine).unwrap();
		let rom = machine.roms().iter().find(|r| r.name() == rom).unwrap();
		let actual = rom.asset_hash();
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_coco.xml"), "coco_scii", 1, &[("cdos", "Disto C-DOS v4.0 for the CoCo 1/2"), ("cdos3", "Disto C-DOS 3 v1.2 for the CoCo 3")])]
	pub fn biossets(
		_index: usize,
		xml: &str,
		machine: &str,
		expected_default_biosset_index: usize,
		expected: &[(&str, &str)],
	) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let machine = db.machines().find(machine).unwrap();
		let actual_default_biosset_index = machine.default_biosset_index();
		let actual = machine
			.biossets()
			.iter()
			.map(|b| (b.name(), b.description()))
			.collect::<Vec<_>>();

		assert_eq!(
			(Some(expected_default_biosset_index), expected),
			(actual_default_biosset_index, actual.as_slice())
		);
	}

	#[test_case(0, include_str!("test_data/listxml_alienar.xml"), "alienar", &[(ChipType::Cpu, "maincpu"), (ChipType::Cpu, "soundcpu"), (ChipType::Audio, "speaker"), (ChipType::Audio, "dac")])]
	#[test_case(1, include_str!("test_data/listxml_fake.xml"), "fake", &[(ChipType::Cpu, "maincpu")])]
	pub fn chips(_index: usize, xml: &str, machine: &str, expected: &[(ChipType, &str)]) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let actual = db
			.machines()
			.find(machine)
			.unwrap()
			.chips()
			.iter()
			.map(|chip| (chip.chip_type(), chip.tag().to_string()))
			.collect::<Vec<_>>();

		let expected = expected
			.iter()
			.map(|(chip_type, tag)| (*chip_type, tag.to_string()))
			.collect::<Vec<_>>();
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_coco.xml"), "coco2b", "ext:fdc:wd17xx:0:525dd", "floppydisk", &["floppy_5_25"],
		&["1dd", "86f", "cqi", "cqm", "d77", "d88", "dfi", "dmk", "dsk", "imd", "jvc", "mfi", "mfm", "os9", "sdf", "td0", "vdk"])]
	#[test_case(1, include_str!("test_data/listxml_c64.xml"), "c64", "exp", "cartridge", &["c64_cart" ,"vic10_cart"],
		&["80", "a0", "crt", "e0"])]
	pub fn devices(
		_index: usize,
		xml: &str,
		machine: &str,
		device_tag: &str,
		expected_type: &str,
		expected_interfaces: &[&str],
		expected_extensions: &[&str],
	) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let device = db
			.machines()
			.find(machine)
			.unwrap()
			.devices()
			.iter()
			.filter(|x| x.tag() == device_tag)
			.exactly_one()
			.map_err(|e| e.to_string())
			.unwrap();
		let actual = (
			device.device_type().to_string(),
			device.interfaces().map(|x| x.to_string()).collect::<Vec<_>>(),
			device.extensions().map(|x| x.to_string()).collect::<Vec<_>>(),
		);

		let expected = (
			expected_type.to_string(),
			expected_interfaces.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
			expected_extensions.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
		);
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_coco.xml"), "coco2b", &["rs232", "ext", "ext:fdc:wd17xx:0", "ext:fdc:wd17xx:1", "ext:fdc:wd17xx:2", "ext:fdc:wd17xx:3"])]
	#[test_case(1, include_str!("test_data/listxml_fake.xml"), "fake", &["ext", "ext:fdcv11:wd17xx:0", "ext:fdcv11:wd17xx:1"])]
	pub fn slots(_index: usize, xml: &str, machine: &str, expected: &[&str]) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let actual = db
			.machines()
			.find(machine)
			.unwrap()
			.slots()
			.iter()
			.map(|s| s.name().to_string())
			.collect::<Vec<_>>();

		let expected = expected.iter().map(|x| x.to_string()).collect::<Vec<_>>();
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_coco.xml"), "coco2b", "ext", Some(16), &[("scii", "coco_scii"), ("cp450_fdc", "cp450_fdc"), ("cd6809_fdc", "cd6809_fdc"), ("sym12", "coco_symphony_twelve")])]
	pub fn slot_options(
		_index: usize,
		xml: &str,
		machine: &str,
		slot: &str,
		expected_default_opt: Option<usize>,
		expected_options: &[(&str, &str)],
	) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let slot = db
			.machines()
			.find(machine)
			.unwrap()
			.slots()
			.iter()
			.find(|x| x.name() == slot)
			.unwrap();

		let actual = slot
			.options()
			.iter()
			.map(|o| (o.name().to_string(), o.devname().to_string()))
			.take(expected_options.len())
			.collect::<Vec<_>>();
		let actual = (slot.default_option_index(), actual);

		let expected = (
			expected_default_opt,
			expected_options
				.iter()
				.map(|x| (x.0.to_string(), x.1.to_string()))
				.collect::<Vec<_>>(),
		);
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_coco.xml"), "coco2b", &[("coco_cart_list", "coco_cart"), ("coco_flop_list", "coco_flop"), ("dragon_cart_list", "dragon_cart")])]
	pub fn machine_software_lists(_index: usize, xml: &str, machine: &str, expected: &[(&str, &str)]) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let actual = db
			.machines()
			.find(machine)
			.unwrap()
			.machine_software_lists()
			.iter()
			.map(|msl| (msl.tag().to_string(), msl.software_list().name().to_string()))
			.collect::<Vec<_>>();

		let expected = expected
			.iter()
			.map(|(tag, name)| (tag.to_string(), name.to_string()))
			.collect::<Vec<_>>();
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("test_data/listxml_coco.xml"), "coco_cart", &["coco", "coco2b", "coco2bh", "coco3"], &[])]
	#[test_case(1, include_str!("test_data/listxml_coco.xml"), "dragon_cart", &[], &["coco", "coco2b", "coco2bh", "cocoh"])]
	pub fn software_lists(
		_index: usize,
		xml: &str,
		software_list_name: &str,
		expected_originals: &[&str],
		expected_compatibles: &[&str],
	) {
		let expected_originals = expected_originals.iter().map(|x| x.to_string()).collect::<Vec<_>>();
		let expected_compatibles = expected_compatibles.iter().map(|x| x.to_string()).collect::<Vec<_>>();

		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let software_list = db
			.software_lists()
			.iter()
			.find(|x| x.name() == software_list_name)
			.expect("Could not find software list");

		let actual_originals = software_list
			.original_for_machines()
			.iter()
			.take(max(expected_originals.len(), expected_compatibles.len()))
			.map(|x| x.name().to_string())
			.collect::<Vec<_>>();
		let actual_compatibles = software_list
			.compatible_for_machines()
			.iter()
			.take(max(expected_originals.len(), expected_compatibles.len()))
			.map(|x| x.name().to_string())
			.collect::<Vec<_>>();
		assert_eq!(
			(expected_originals, expected_compatibles),
			(actual_originals, actual_compatibles)
		);
	}

	#[test_case(0, include_str!("test_data/listxml_coco.xml"), "coco2b", &[("beckerport", 1, 0), ("ctrl_sel", 15, 1), ("ctrl_sel", 240, 1), ("dwsock:drivewire_port", 65535, 4), ("hires_intf", 7, 0), ("rs232:rs_printer:RS232_DATABITS", 255, 3), ("rs232:rs_printer:RS232_PARITY", 255, 0), ("rs232:rs_printer:RS232_RXBAUD", 255, 7), ("rs232:rs_printer:RS232_STOPBITS", 255, 1), ("vdg:artifacting", 3, 1)])]
	#[test_case(1, include_str!("test_data/listxml_coco.xml"), "coco3", &[("beckerport", 1, 0), ("ctrl_sel", 15, 1), ("ctrl_sel", 240, 1), ("dwsock:drivewire_port", 65535, 4), ("gime:artifacting", 3, 1), ("hires_intf", 7, 0), ("rs232:rs_printer:RS232_DATABITS", 255, 3), ("rs232:rs_printer:RS232_PARITY", 255, 0), ("rs232:rs_printer:RS232_RXBAUD", 255, 7), ("rs232:rs_printer:RS232_STOPBITS", 255, 1), ("screen_config", 1, 0)])]
	fn configurations(_index: usize, xml: &str, machine_name: &str, expected: &[(&str, u32, usize)]) {
		let db = InfoDb::from_listxml_output(xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let configs = db.machines().find(machine_name).unwrap().configurations();

		let actual = configs
			.iter()
			.map(|c| (c.tag(), c.mask(), c.default_setting_index().unwrap()))
			.collect::<Vec<_>>();
		assert_eq!(expected, actual.as_slice());
	}
}
