use std::borrow::Cow;
use std::iter::once;
use std::ops::Range;
use std::rc::Rc;

use itertools::Itertools;

use crate::info::InfoDb;
use crate::info::View;
use crate::status;
use crate::status::Status;

pub struct DevicesImagesConfig {
	info_db: Rc<InfoDb>,
	machine_index: Option<usize>,
	entries: Vec<InternalEntry>,
}

#[derive(Debug)]
struct InternalEntry {
	tag: String,
	subtag_start: usize,
	indent: usize,
	details: InternalEntryDetails,
}

#[derive(Debug)]
enum InternalEntryDetails {
	Slot { current_option_index: Option<usize> },
	Image { filename: Option<String> },
}

#[derive(Debug)]
pub struct Entry<'a> {
	pub tag: &'a str,
	pub subtag: &'a str,
	pub indent: usize,
	pub details: EntryDetails<'a>,
}

#[derive(Debug)]
pub enum EntryDetails<'a> {
	Slot {
		options: Vec<EntryOption<'a>>,
		current_option_index: usize,
	},
	Image {
		filename: Option<&'a str>,
	},
}

#[derive(Debug)]
pub struct EntryOption<'a> {
	pub name: Option<Cow<'a, str>>,
	pub description: Option<Cow<'a, str>>,
}

impl DevicesImagesConfig {
	pub fn new(info_db: Rc<InfoDb>) -> Self {
		Self {
			info_db,
			machine_index: None,
			entries: Vec::new(),
		}
	}

	pub fn entry_count(&self) -> usize {
		self.entries.len()
	}

	pub fn entry(&self, index: usize) -> Option<Entry> {
		let internal_entry = self.entries.get(index)?;
		let machine = self.info_db.machines().get(self.machine_index?)?;

		let details = match &internal_entry.details {
			InternalEntryDetails::Slot { current_option_index } => {
				let info = machine
					.slots()
					.iter()
					.find(|x| x.name().as_ref() == internal_entry.tag)
					.unwrap();
				let none_option = EntryOption {
					name: None,
					description: None,
				};
				let options = once(none_option)
					.chain(info.options().iter().map(|slot_option| {
						let devmachine = self.info_db.machines().find(&slot_option.devname()).unwrap();
						let name = Some(devmachine.name().into());
						let description = Some(devmachine.description().into());
						EntryOption { name, description }
					}))
					.collect::<Vec<_>>();
				let current_option_index = current_option_index.map(|x| x + 1).unwrap_or(0);
				EntryDetails::Slot {
					options,
					current_option_index,
				}
			}
			InternalEntryDetails::Image { filename } => {
				let filename = filename.as_deref();
				EntryDetails::Image { filename }
			}
		};

		let entry = Entry {
			tag: &internal_entry.tag,
			subtag: &internal_entry.tag[internal_entry.subtag_start..],
			indent: internal_entry.indent,
			details,
		};
		Some(entry)
	}

	pub fn update_status(&self, status: &Status) -> (Self, Option<Range<usize>>) {
		// note that this logic won't error; this is because we expect the InfoDB and Status data
		// to be in harmony; we really need a status validation step
		let info_db = self.info_db.clone();
		let new_status = internal_update_status(info_db, status);
		(new_status, None)
	}
}

fn internal_update_status(info_db: Rc<InfoDb>, status: &Status) -> DevicesImagesConfig {
	let Some(running) = status.running.as_ref() else {
		return DevicesImagesConfig::new(info_db);
	};
	let machine_index = info_db
		.machines()
		.find_index(&running.machine_name)
		.unwrap_or_else(|| panic!("Unknown machine {:?}", running.machine_name));
	let machine = info_db.machines().get(machine_index).unwrap();

	// we wish to merge slot and status information into a single hierarchy; before we can
	// get the hierarchy, we need to compile them into a unified list with each of the tags
	// split into their parts

	// first, an enum that can be a Slot or an Image from a status update
	enum StatusEntity<'a> {
		Slot(&'a status::Slot),
		Image(&'a status::Image),
	}
	impl StatusEntity<'_> {
		pub fn tag(&self) -> &'_ str {
			match self {
				Self::Slot(x) => &x.name,
				Self::Image(x) => &x.tag,
			}
		}
	}

	// then merge info about slots and images and sort the results
	let slot_status_iter = running.slots.iter().map(StatusEntity::Slot);
	let image_status_iter = running.images.iter().map(StatusEntity::Image);
	let statuses = slot_status_iter
		.chain(image_status_iter)
		.sorted_by(|a, b| Ord::cmp(&a.tag(), &b.tag()))
		.collect::<Vec<_>>();

	// build the hierarchy
	let hierarchy = hierarchicalize(statuses.iter().map(|x| x.tag()), ':');

	// now that we've gathered the slots/images and built the hierarchy, we can build out all of the entries
	let entries = statuses
		.into_iter()
		.zip(hierarchy)
		.filter_map(|(status, (indent, subtag_start))| {
			let details = match status {
				StatusEntity::Slot(status) if status.has_selectable_options => {
					let info = machine
						.slots()
						.iter()
						.find(|x| x.name().as_ref() == status.name)
						.unwrap_or_else(|| panic!("Unknown slot {:?}", status.name));

					let current_option_index = status.current_option.map(|index| {
						let option_name = status
							.options
							.get(index)
							.unwrap_or_else(|| panic!("Current option index {:?} is out of range", index))
							.name
							.as_str();
						info.options()
							.iter()
							.position(|x| x.name() == option_name)
							.unwrap_or_else(|| panic!("Unknown slot option {:?}", option_name))
					});
					Some(InternalEntryDetails::Slot { current_option_index })
				}
				StatusEntity::Slot(_) => None,
				StatusEntity::Image(status) => {
					let filename = status.filename.clone();
					Some(InternalEntryDetails::Image { filename })
				}
			}?;

			let tag = status.tag().to_string();
			let entry = InternalEntry {
				tag,
				subtag_start,
				indent,
				details,
			};
			Some(entry)
		})
		.collect::<Vec<_>>();

	// return the new config
	DevicesImagesConfig {
		info_db,
		machine_index: Some(machine_index),
		entries,
	}
}

fn hierarchicalize<'a>(tag_iter: impl Iterator<Item = &'a str>, delim: char) -> Vec<(usize, usize)> {
	let data = tag_iter
		.map(move |tag| tag.split(delim).inspect(|s| assert!(!s.is_empty())).collect::<Vec<_>>())
		.collect::<Vec<_>>();
	internal_hierarchicalize(&data, 0, 0)
		.zip(&data)
		.map(|((indent, subtag_part_index), tag_parts)| {
			let subtag_start =
				tag_parts[..subtag_part_index].iter().map(|x| x.len()).sum::<usize>() + subtag_part_index;
			(indent, subtag_start)
		})
		.collect::<Vec<_>>()
}

fn internal_hierarchicalize<T>(
	data: &[impl AsRef<[T]>],
	depth: usize,
	base_len: usize,
) -> impl Iterator<Item = (usize, usize)> + '_
where
	T: PartialEq,
{
	data.iter()
		.enumerate()
		.map(|(idx, line)| (idx..idx + 1, line))
		.coalesce(move |(range_a, line_a), (range_b, line_b)| {
			let line_a_ref = line_a.as_ref();
			let line_b_ref = line_b.as_ref();
			if line_b_ref.len() > line_a_ref.len() && line_a_ref[depth..] == line_b_ref[depth..line_a_ref.len()] {
				Ok((range_a.start..range_b.end, line_a))
			} else {
				Err(((range_a, line_a), (range_b, line_b)))
			}
		})
		.flat_map(move |(range, line)| {
			let next_range = range.start + 1..range.end;
			let next_depth = depth + 1;
			let next_base_len = line.as_ref().len();
			once((depth, base_len))
				.chain(internal_hierarchicalize(&data[next_range], next_depth, next_base_len))
				.collect::<Vec<_>>()
		})
}

#[cfg(test)]
mod test {
	use std::rc::Rc;

	use test_case::test_case;

	use crate::info::InfoDb;
	use crate::status::Status;
	use crate::status::Update;

	use super::DevicesImagesConfig;

	#[test_case(0, include_str!("info/test_data/listxml_c64.xml"), include_str!("status/test_data/status_mame0273_c64_1.xml"))]
	fn test(_index: usize, info_xml: &str, status_xml: &str) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| false)
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		// build the status
		let mut status = Status::default();
		let update = Update::parse(status_xml.as_bytes()).unwrap();
		status.merge(update);

		// now try to create a config
		let config = DevicesImagesConfig::new(info_db);
		let _ = config.update_status(&status);
	}

	#[test_case(0, &["alpha", "alpha:bravo", "alpha:charlie", "delta", "echo", "echo:foxtrot"], &["alpha", "-bravo", "-charlie", "delta", "echo", "-foxtrot"])]
	#[test_case(1, &["alpha", "alpha:foo:bar:bravo", "alpha:foo:bar:charlie", "alpha:foo:bar:charlie:delta", "echo", "echo:foxtrot"], &["alpha", "-foo:bar:bravo", "-foo:bar:charlie", "--delta", "echo", "-foxtrot"])]
	#[test_case(2, &["alpha", "alpha:bravo:charlie:delta:echo", "alpha:bravo:charlie:delta:echo:foxtrot", "alpha:bravo:charlie:delta:echo:golf"], &["alpha", "-bravo:charlie:delta:echo", "--foxtrot", "--golf"])]
	fn hierarchicalize(_index: usize, data: &[&str], expected: &[&str]) {
		let actual = super::hierarchicalize(data.iter().copied(), ':')
			.into_iter()
			.zip(data.iter())
			.map(|((indent, subtag_start), tag)| {
				format!(
					"{}{}",
					(0..indent).map(|_| '-').collect::<String>(),
					&tag[subtag_start..]
				)
			})
			.collect::<Vec<_>>();
		assert_eq!(expected, actual.as_slice())
	}
}
