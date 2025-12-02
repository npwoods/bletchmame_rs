use std::borrow::Cow;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::ops::ControlFlow;
use std::rc::Rc;

use anyhow::Error;
use anyhow::Result;
use more_asserts::assert_le;
use tracing::debug;
use tracing::info;

use crate::info::Device;
use crate::info::InfoDb;
use crate::info::Machine;
use crate::info::Slot;
use crate::info::View;

#[derive(Clone)]
pub struct MachineConfig {
	pub info_db: Rc<InfoDb>,
	pub machine_index: usize,
	slots: Rc<[SlotData]>,
}

#[derive(Clone, Debug)]
enum SlotData {
	Unset,
	Set {
		option_index: usize,
		config: Rc<MachineConfig>,
	},
	Ignore,
}

impl SlotData {
	pub fn option_index(&self) -> Option<usize> {
		match self {
			Self::Unset => None,
			Self::Set { option_index, .. } => Some(*option_index),
			Self::Ignore => unreachable!(),
		}
	}
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Machine {0:?}: Could not find slot {1:?}")]
	UnknownSlot(String, String),
	#[error("Machine {0:?} Slot {1:?}: Traversal expected option {2:?} but rest of tag is {3:?}")]
	WrongOption(String, String, Option<String>, String),
	#[error("Machine {0:?} Slot {1:?}: Cannot find option {2:?}")]
	CannotFindSlotOption(String, String, String),
	#[error("Machine {0:?}: Cannot set option on machine")]
	CantSetOptionOnMachine(String),
	#[error("Cannot find device {0}")]
	CannotFindDevice(String),
	#[error("Expected tag {0:?} on machine {1:?} to be {2:?} but was {3:?}")]
	IncorrectTagType(String, String, TagType, TagType),
}

#[derive(Clone, Debug)]
pub enum TagType {
	Machine,
	Slot,
}

#[derive(Clone, Debug)]
pub enum TagLookup<'a> {
	Machine(Machine<'a>),
	Slot(Machine<'a>, Slot<'a>),
}

impl From<&TagLookup<'_>> for TagType {
	fn from(value: &TagLookup<'_>) -> Self {
		match value {
			TagLookup::Machine(_) => TagType::Machine,
			TagLookup::Slot(_, _) => TagType::Slot,
		}
	}
}

impl MachineConfig {
	pub fn new(info_db: Rc<InfoDb>, machine_index: usize) -> Self {
		let machine = info_db.machines().get(machine_index).unwrap();
		let slots = machine
			.slots()
			.iter()
			.map(|slot| {
				// the InfoDB contains child slots (e.g. - `ext:fdc:wd17xx:0` on `coco2b`), and we want to skip
				// over these
				let ignore = machine
					.slots()
					.iter()
					.any(|x| strip_tag_prefix(slot.name(), x.name()).is_some_and(|x| !x.is_empty()));
				if ignore {
					SlotData::Ignore
				} else {
					// when we get the default index, we need to ensure that it references an actual machine
					let option_and_machine_index = slot.default_option_index().and_then(|option_index| {
						let machine_name = slot.options().get(option_index).unwrap().devname();
						info_db
							.machines()
							.find_index(machine_name)
							.ok()
							.map(|machine_index| (option_index, machine_index))
					});

					if let Some((option_index, machine_index)) = option_and_machine_index {
						let config = Self::new(info_db.clone(), machine_index);
						let config = Rc::new(config);
						SlotData::Set { option_index, config }
					} else {
						SlotData::Unset
					}
				}
			})
			.collect();

		Self {
			info_db,
			machine_index,
			slots,
		}
	}

	pub fn from_machine_index(info_db: Rc<InfoDb>, machine_index: usize) -> Self {
		let opts: &[(&str, Option<&str>)] = Default::default();
		Self::from_machine_index_and_slots(info_db, machine_index, opts).unwrap()
	}

	pub fn from_machine_name_and_slots(
		info_db: Rc<InfoDb>,
		machine_name: &str,
		opts: &[(impl AsRef<str>, Option<impl AsRef<str>>)],
	) -> Result<Self> {
		let machine_index = info_db.machines().find_index(machine_name)?;
		Self::from_machine_index_and_slots(info_db, machine_index, opts)
	}

	pub fn from_machine_index_and_slots(
		info_db: Rc<InfoDb>,
		machine_index: usize,
		opts: &[(impl AsRef<str>, Option<impl AsRef<str>>)],
	) -> Result<Self> {
		assert_le!(machine_index, info_db.machines().len());
		let mut result = Self::new(info_db, machine_index);

		for (tag, new_option_name) in opts {
			let tag = tag.as_ref();
			let new_option_name = new_option_name.as_ref().map(|x| x.as_ref());
			result = result.set_slot_option(tag, new_option_name)?.unwrap_or(result);
		}

		Ok(result)
	}

	pub fn machine(&self) -> Machine<'_> {
		self.info_db.machines().get(self.machine_index).unwrap()
	}

	pub fn lookup_tag(&self, tag: &str) -> Result<TagLookup<'_>> {
		match self.traverse_tag(tag)? {
			ControlFlow::Continue((slot_index, next_tag)) => {
				let SlotData::Set { config, .. } = &self.slots[slot_index] else {
					unreachable!();
				};
				config.lookup_tag(next_tag)
			}
			ControlFlow::Break(slot_index) => {
				let machine = self.machine();
				let result = if let Some(slot_index) = slot_index {
					TagLookup::Slot(machine, machine.slots().get(slot_index).unwrap())
				} else {
					TagLookup::Machine(machine)
				};
				Ok(result)
			}
		}
	}

	pub fn lookup_slot_tag(&self, tag: &str) -> Result<(Machine<'_>, Slot<'_>)> {
		let result = self.lookup_tag(tag)?;
		let TagLookup::Slot(machine, slot) = result else {
			let tag = tag.to_string();
			let machine_name = self.machine().name().to_string();
			let error = ThisError::IncorrectTagType(tag, machine_name, TagType::Slot, (&result).into());
			return Err(error.into());
		};
		Ok((machine, slot))
	}

	pub fn lookup_device_tag(&self, tag: &str) -> Result<(Machine<'_>, Device<'_>)> {
		let (machine_tag, device_tag) = tag.rsplit_once(':').unwrap_or(("", tag));
		let result = self.lookup_tag(machine_tag)?;
		let TagLookup::Machine(machine) = result else {
			let tag = tag.to_string();
			let machine_name = self.machine().name().to_string();
			let error = ThisError::IncorrectTagType(tag, machine_name, TagType::Machine, (&result).into());
			return Err(error.into());
		};

		let device = machine
			.devices()
			.iter()
			.find(|x| x.tag() == device_tag)
			.ok_or_else(|| Error::from(ThisError::CannotFindDevice(device_tag.to_string())))?;
		Ok((machine, device))
	}

	pub fn set_slot_option(&self, tag: &str, new_option_name: Option<&str>) -> Result<Option<Self>> {
		info!(tag=?tag, new_option_name=?new_option_name, "MachineConfig::set_option()");

		let machine = self.machine();
		let changes = match self.traverse_tag(tag)? {
			ControlFlow::Continue((slot_index, next_tag)) => {
				let SlotData::Set {
					option_index, config, ..
				} = &self.slots[slot_index]
				else {
					unreachable!();
				};
				config
					.set_slot_option(next_tag, new_option_name)?
					.map(|new_config| (slot_index, Some((*option_index, new_config))))
			}
			ControlFlow::Break(Some(slot_index)) => {
				let slot = machine.slots().get(slot_index).unwrap();
				let old_option_index = self.slots[slot_index].option_index();
				let new_option_index = new_option_name
					.map(|new_option_name| {
						slot.options()
							.iter()
							.position(|opt| opt.name() == new_option_name)
							.ok_or_else(|| {
								ThisError::CannotFindSlotOption(
									machine.name().to_string(),
									slot.name().to_string(),
									new_option_name.to_string(),
								)
							})
					})
					.transpose()?;
				if old_option_index != new_option_index {
					let new_slot_data = new_option_index.map(|option_index| {
						let new_option = slot.options().get(option_index).unwrap();
						let machine_index = self
							.info_db
							.clone()
							.machines()
							.find_index(new_option.devname())
							.unwrap();
						let new_config = Self::new(self.info_db.clone(), machine_index);
						(option_index, new_config)
					});
					Some((slot_index, new_slot_data))
				} else {
					// no change, that was easy!
					None
				}
			}
			ControlFlow::Break(None) => {
				return Err(ThisError::CantSetOptionOnMachine(machine.name().to_string()).into());
			}
		};

		let result = changes.map(|(slot_index, new_slot_data)| {
			let new_slot_data = if let Some((option_index, config)) = new_slot_data {
				let config = Rc::new(config);
				SlotData::Set { option_index, config }
			} else {
				SlotData::Unset
			};
			let mut new_slot_data = Some(new_slot_data);

			let slots = self
				.slots
				.iter()
				.enumerate()
				.map(|(index, slot_data)| {
					if index == slot_index {
						new_slot_data.take().unwrap()
					} else {
						slot_data.clone()
					}
				})
				.collect();

			let info_db = self.info_db.clone();
			let machine_index = self.machine_index;
			Self {
				info_db,
				machine_index,
				slots,
			}
		});
		Ok(result)
	}

	fn traverse_tag<'a>(&self, tag: &'a str) -> Result<ControlFlow<Option<usize>, (usize, &'a str)>> {
		let result = if tag.is_empty() {
			// tag is empty - break with no slot (this was a machine lookup)
			ControlFlow::Break(None)
		} else {
			// attempt to find the slot on this machine
			let machine = self.machine();
			let (slot_index, slot, next_tag) = machine
				.slots()
				.iter()
				.enumerate()
				.filter_map(|(slot_index, slot)| {
					strip_tag_prefix(tag, slot.name()).map(|next_tag| (slot_index, slot, next_tag))
				})
				.next()
				.ok_or_else(|| ThisError::UnknownSlot(machine.name().to_string(), tag.to_string()))?;

			// we found the slot - is this it?
			if next_tag.is_empty() {
				// we're at the end - break with a slot
				ControlFlow::Break(Some(slot_index))
			} else {
				// we're not at the end; drill down by returning `ControlFlow::Continue`
				let expected_option = self.slots[slot_index]
					.option_index()
					.map(|x| slot.options().get(x).unwrap().name());
				let next_tag = expected_option
					.and_then(|x| strip_tag_prefix(next_tag, x))
					.ok_or_else(|| {
						ThisError::WrongOption(
							machine.name().to_string(),
							slot.name().to_string(),
							expected_option.map(str::to_string),
							next_tag.to_string(),
						)
					})?;
				ControlFlow::Continue((slot_index, next_tag))
			}
		};
		Ok(result)
	}

	pub fn visit_slots<'a>(&'a self, mut callback: impl FnMut(usize, &str, Machine<'a>, Slot<'a>, Option<usize>)) {
		self.internal_visit_slots(&mut callback, "", 0);
	}

	fn internal_visit_slots<'a>(
		&'a self,
		callback: &mut impl FnMut(usize, &str, Machine<'a>, Slot<'a>, Option<usize>),
		base_tag: &str,
		depth: usize,
	) {
		let machine = self.machine();
		for (slot, slot_data) in machine.slots().iter().zip(self.slots.iter()) {
			if !matches!(slot_data, SlotData::Ignore) {
				// invoke the callback
				callback(depth, base_tag, machine, slot, slot_data.option_index());

				if let SlotData::Set { option_index, config } = &slot_data {
					let base_tag = format!(
						"{}{}:{}:",
						base_tag,
						slot.name(),
						slot.options().get(*option_index).unwrap().name()
					);
					config.internal_visit_slots(callback, &base_tag, depth + 1);
				}
			}
		}
	}

	pub fn changed_slots<'a>(&'a self, base: Option<&Self>) -> Vec<(String, Option<&'a str>)> {
		debug!(?self, ?base, "MachineConfig::changed_slots()");

		let mut results = Vec::new();
		self.internal_changed_slots(base, "", &mut |slot, opt| {
			results.push((slot, opt));
		});
		results
	}

	fn internal_changed_slots<'a>(
		&'a self,
		base: Option<&Self>,
		base_tag: &str,
		emit: &mut impl FnMut(String, Option<&'a str>),
	) {
		// if we were not passed a `base`, we're being asked to check against the default configuration; it
		// needs to be constructed
		let base = if let Some(base) = base {
			assert_eq!(self.machine_index, base.machine_index);
			Cow::Borrowed(base)
		} else {
			let base = Self::new(self.info_db.clone(), self.machine_index);
			Cow::Owned(base)
		};

		// compare all our slots and the base
		for ((ent, base_ent), slot) in self
			.slots
			.iter()
			.zip(base.slots.as_ref())
			.zip(self.machine().slots().iter())
			.filter(|((ent, _), _)| !matches!(ent, SlotData::Ignore))
		{
			// determine the tag
			let slot_tag = if !base_tag.is_empty() {
				format!("{}:{}", base_tag, slot.name())
			} else {
				slot.name().to_string()
			};

			// is this particular slot changed when compared with the base?  if so, emit it
			if ent.option_index() != base_ent.option_index() {
				let option = ent.option_index().map(|x| slot.options().get(x).unwrap().name());
				emit(slot_tag.clone(), option);
			}

			// if an option is specified, we need to recurse into that slot
			if let SlotData::Set { option_index, config } = &ent {
				let child_config = config.as_ref();
				let child_base_config = if let SlotData::Set {
					option_index: base_option_index,
					config: base_config,
				} = &base_ent
				{
					(base_option_index == option_index).then_some(base_config.as_ref())
				} else {
					None
				};
				let slot_tag = format!("{}:{}", slot_tag, slot.options().get(*option_index).unwrap().name());
				child_config.internal_changed_slots(child_base_config, &slot_tag, emit)
			}
		}
	}
}

impl Debug for MachineConfig {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		let machine = self.machine();
		let mut debug = f.debug_struct("MachineConfig");
		debug.field("machine", &machine.name());

		for (slot, slot_data) in machine.slots().iter().zip(self.slots.iter()) {
			match slot_data {
				SlotData::Unset => debug.field(slot.name(), &"<<unset>>"),
				SlotData::Set { option_index, config } => {
					let option = slot.options().get(*option_index).unwrap();
					debug.field(slot.name(), &(option.name(), config.as_ref()))
				}
				SlotData::Ignore => debug.field(slot.name(), &"<<ignore>>"),
			};
		}

		debug.finish()
	}
}

fn strip_tag_prefix<'a>(tag: &'a str, target: &str) -> Option<&'a str> {
	tag.strip_prefix(target)
		.and_then(|x| if x.is_empty() { Some(x) } else { x.strip_prefix(":") })
}

#[cfg(test)]
mod test {
	use std::ops::ControlFlow;
	use std::rc::Rc;

	use assert_matches::assert_matches;
	use test_case::test_case;

	use crate::info::InfoDb;

	use super::MachineConfig;
	use super::TagLookup;
	use super::ThisError;

	#[test_case(0, include_str!("info/test_data/listxml_coco.xml"), "coco2b", &[])]
	#[test_case(1, include_str!("info/test_data/listxml_coco.xml"), "coco2b", &[("ext", Some("multi"))])]
	#[test_case(2, include_str!("info/test_data/listxml_coco.xml"), "coco2b", &[("ext", Some("multi")), ("ext:multi:slot4:fdc:wd17xx:1", None)])]
	fn from_machine_name_and_slots(_index: usize, info_xml: &str, machine_name: &str, opts: &[(&str, Option<&str>)]) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		let actual = MachineConfig::from_machine_name_and_slots(info_db, machine_name, opts);
		assert_matches!(actual, Ok(_));
	}

	#[test_case(0, include_str!("info/test_data/listxml_coco.xml"), "coco2b", "ext", Some("fdc"), Ok(false))]
	#[test_case(1, include_str!("info/test_data/listxml_coco.xml"), "coco2b", "ext", Some("multi"), Ok(true))]
	#[test_case(2, include_str!("info/test_data/listxml_coco.xml"), "coco2b", "ext:fdc:wd17xx:0", None, Ok(true))]
	#[test_case(3, include_str!("info/test_data/listxml_coco.xml"), "coco2b", "ext:fdc:wd17xx:0", Some("525dd"), Ok(false))]
	fn set_slot_option(
		_index: usize,
		info_xml: &str,
		machine_name: &str,
		tag: &str,
		new_option_name: Option<&str>,
		expected: Result<bool, String>,
	) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		let machine_index = info_db.machines().find_index(machine_name).unwrap();
		let config = MachineConfig::new(info_db, machine_index);
		let actual = config
			.set_slot_option(tag, new_option_name)
			.map(|x| x.is_some())
			.map_err(|e| e.to_string());

		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, "", Ok(("coco2b", None)))]
	#[test_case(1, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, "ext", Ok(("coco2b", Some("ext"))))]
	#[test_case(2, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, "ext:fdc", Ok(("coco_fdc", None)))]
	#[test_case(3, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, "ext:fdc:wd17xx:0", Ok(("coco_fdc", Some("wd17xx:0"))))]
	#[test_case(4, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, "ext:fdc:wd17xx:1", Ok(("coco_fdc", Some("wd17xx:1"))))]
	#[test_case(5, include_str!("info/test_data/listxml_coco.xml"), "coco2b", Some(("ext", Some("multi"))), "ext:multi:slot4:fdc:wd17xx:1", Ok(("coco_fdc", Some("wd17xx:1"))))]
	fn set_slot_option_and_lookup_tag(
		_index: usize,
		info_xml: &str,
		machine_name: &str,
		set_option: Option<(&str, Option<&str>)>,
		tag: &str,
		expected: Result<(&str, Option<&str>), ThisError>,
	) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		// create the initial config
		let machine_index = info_db.machines().find_index(machine_name).unwrap();
		let mut config = MachineConfig::new(info_db, machine_index);

		// set the option if specified
		if let Some((tag, new_option_name)) = set_option {
			config = config.set_slot_option(tag, new_option_name).unwrap().unwrap_or(config);
			assert_eq!(
				machine_index, config.machine_index,
				"MachineConfig::set_slot_option() changed the machine_index"
			);
		}

		// perform the tag lookup
		let actual = match config.lookup_tag(tag) {
			Ok(TagLookup::Machine(machine)) => Ok((machine.name().to_string(), None)),
			Ok(TagLookup::Slot(machine, slot)) => Ok((machine.name().to_string(), Some(slot.name().to_string()))),
			Err(e) => Err(e.to_string()),
		};

		// and validate
		let expected = expected
			.map(|(machine_name, slot_name)| (machine_name.to_string(), slot_name.map(|x| x.to_string())))
			.map_err(|e| e.to_string());
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, &[], &[])]
	#[test_case(1, include_str!("info/test_data/listxml_coco.xml"), "coco2b", Some(&[]), &[], &[])]
	#[test_case(2, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, &[("ext", Some("multi"))], &[("ext", Some("multi"))])]
	#[test_case(3, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, &[("ext", Some("multi")), ("ext:multi:slot4:fdc:wd17xx:1", None)], &[("ext", Some("multi")), ("ext:multi:slot4:fdc:wd17xx:1", None)])]
	fn changed_slots(
		_index: usize,
		info_xml: &str,
		machine_name: &str,
		opts1: Option<&[(&str, Option<&str>)]>,
		opts2: &[(&str, Option<&str>)],
		expected: &[(&str, Option<&str>)],
	) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| ControlFlow::Continue(()))
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		// create the config for `opts1` (if `Some`)
		let config1 = opts1.map(|opts1| {
			let info_db = info_db.clone();
			MachineConfig::from_machine_name_and_slots(info_db, machine_name, opts1).unwrap()
		});

		// create the config for `opts2`
		let config2 = MachineConfig::from_machine_name_and_slots(info_db, machine_name, opts2).unwrap();

		// get the changed slots
		let actual = config2.changed_slots(config1.as_ref());

		// and validate the changes
		let expected = expected
			.iter()
			.map(|(slot, opt)| (slot.to_string(), *opt))
			.collect::<Vec<_>>();
		assert_eq!(expected, actual);
	}

	#[test_case(0, "alpha:bravo:charlie", "alpha", Some("bravo:charlie"))]
	#[test_case(1, "alpha:bravo:charlie", "alpha:bravo", Some("charlie"))]
	#[test_case(2, "alpha:bravo:charlie", "alpha:bravo:charlie", Some(""))]
	#[test_case(3, "alpha:bravo:charlie", "delta", None)]
	#[test_case(4, "alpha:bravo:charlie", "alp", None)]
	#[test_case(5, "alpha:bravo:charlie", "alpha:bra", None)]
	pub fn strip_tag_prefix(_index: usize, tag: &str, target: &str, expected: Option<&str>) {
		let actual = super::strip_tag_prefix(tag, target);
		assert_eq!(expected, actual);
	}
}
