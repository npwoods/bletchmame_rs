use std::borrow::Cow;
use std::ops::ControlFlow;
use std::rc::Rc;

use anyhow::Result;
use tracing::event;
use tracing::Level;

use crate::info::InfoDb;
use crate::info::Machine;
use crate::info::Slot;
use crate::info::View;

const LOG: Level = Level::DEBUG;

#[derive(Clone, Debug)]
pub struct MachineConfig {
	info_db: Rc<InfoDb>,
	pub machine_index: usize,
	slots: Rc<[Option<SlotData>]>,
}

#[derive(Clone, Debug)]
struct SlotData {
	option_index: usize,
	config: Rc<MachineConfig>,
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
}

impl MachineConfig {
	pub fn new(info_db: Rc<InfoDb>, machine_index: usize) -> Self {
		let machine = info_db.machines().get(machine_index).unwrap();
		let slots = machine
			.slots()
			.iter()
			.map(|slot| {
				slot.default_option_index().map(|option_index| {
					let machine_name = slot.options().get(option_index).unwrap().devname();
					let machine_index = info_db.machines().find_index(&machine_name).unwrap();
					let config = Self::new(info_db.clone(), machine_index);
					let config = Rc::new(config);
					SlotData { option_index, config }
				})
			})
			.collect();

		Self {
			info_db,
			machine_index,
			slots,
		}
	}

	pub fn machine(&self) -> Machine<'_> {
		self.info_db.machines().get(self.machine_index).unwrap()
	}

	pub fn lookup_tag(&self, tag: &str) -> Result<(Machine<'_>, Option<Slot<'_>>)> {
		match self.traverse_tag(tag)? {
			ControlFlow::Continue((slot_index, next_tag)) => {
				self.slots[slot_index].as_ref().unwrap().config.lookup_tag(next_tag)
			}
			ControlFlow::Break(slot_index) => {
				let machine = self.machine();
				let slot = slot_index.map(|index| machine.slots().get(index).unwrap());
				Ok((machine, slot))
			}
		}
	}

	pub fn set_slot_option(&self, tag: &str, new_option_name: Option<&str>) -> Result<Option<Self>> {
		event!(LOG, "MachineConfig::set_option(): {tag:?} {new_option_name:?}");

		let machine = self.machine();
		let changes = match self.traverse_tag(tag)? {
			ControlFlow::Continue((slot_index, next_tag)) => {
				let slot_data = self.slots[slot_index].as_ref().unwrap();
				slot_data
					.config
					.set_slot_option(next_tag, new_option_name)?
					.map(|new_config| (slot_index, Some((slot_data.option_index, new_config))))
			}
			ControlFlow::Break(Some(slot_index)) => {
				let slot = machine.slots().get(slot_index).unwrap();
				let old_option_index = self.slots[slot_index].as_ref().map(|x| x.option_index);
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
							.find_index(&new_option.devname())
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
			let mut new_slot_data = new_slot_data.map(|(option_index, config)| {
				let config = Rc::new(config);
				SlotData { option_index, config }
			});
			let slots = self
				.slots
				.iter()
				.enumerate()
				.map(|(index, slot_data)| {
					if index == slot_index {
						new_slot_data.take()
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
					strip_tag_prefix(tag, &slot.name()).map(|next_tag| (slot_index, slot, next_tag))
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
					.as_ref()
					.map(|x| slot.options().get(x.option_index).unwrap().name());
				let next_tag = expected_option
					.and_then(|x| strip_tag_prefix(next_tag, x.as_ref()))
					.ok_or_else(|| {
						ThisError::WrongOption(
							machine.name().to_string(),
							slot.name().to_string(),
							expected_option.as_deref().map(str::to_string),
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
			let option_index = slot_data.as_ref().map(|x| x.option_index);
			callback(depth, base_tag, machine, slot, option_index);

			if let Some(slot_data) = slot_data.as_ref() {
				let base_tag = format!(
					"{}{}:{}:",
					base_tag,
					slot.name(),
					slot.options().get(slot_data.option_index).unwrap().name()
				);
				slot_data.config.internal_visit_slots(callback, &base_tag, depth + 1);
			}
		}
	}

	pub fn changed_slots(&self, base: Option<&Self>) -> Vec<(String, Option<String>)> {
		let mut results = Vec::new();
		self.internal_changed_slots(base, "", &mut |slot, opt| {
			results.push((slot.to_string(), opt.map(str::to_string)));
		});
		results
	}

	fn internal_changed_slots(&self, base: Option<&Self>, base_tag: &str, emit: &mut impl FnMut(&str, Option<&str>)) {
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
		{
			// determine the tag
			let (ent, base_ent) = (ent.as_ref(), base_ent.as_ref());
			let slot_tag = if !base_tag.is_empty() {
				format!("{}:{}", base_tag, slot.name())
			} else {
				slot.name().to_string()
			};

			// is this particular slot changed when compared with the base?  if so, emit it
			if ent.map(|x| x.option_index) != base_ent.map(|x| x.option_index) {
				let option = ent.map(|x| slot.options().get(x.option_index).unwrap().name());
				let option = option.as_ref().map(|x| x.as_ref());
				emit(&slot_tag, option);
			}

			// if an option is specified, we need to recurse into that slot
			if let Some(ent) = ent {
				let child_config = ent.config.as_ref();
				let child_base_config =
					base_ent.and_then(|x| (ent.option_index == x.option_index).then_some(x.config.as_ref()));
				child_config.internal_changed_slots(child_base_config, &slot_tag, emit)
			}
		}
	}
}

fn strip_tag_prefix<'a>(tag: &'a str, target: &str) -> Option<&'a str> {
	tag.strip_prefix(target)
		.and_then(|x| if x.is_empty() { Some(x) } else { x.strip_prefix(":") })
}

#[cfg(test)]
mod test {
	use std::rc::Rc;

	use test_case::test_case;

	use crate::info::InfoDb;

	use super::MachineConfig;
	use super::ThisError;

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
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| false)
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
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| false)
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
		let actual = config
			.lookup_tag(tag)
			.map(|(machine, slot)| (machine.name().to_string(), slot.map(|x| x.name().to_string())))
			.map_err(|e| e.to_string());

		// and validate
		let expected = expected
			.map(|(machine_name, slot_name)| (machine_name.to_string(), slot_name.map(|x| x.to_string())))
			.map_err(|e| e.to_string());
		assert_eq!(expected, actual);
	}

	#[test_case(0, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, &[], &[])]
	#[test_case(1, include_str!("info/test_data/listxml_coco.xml"), "coco2b", Some(&[]), &[], &[])]
	#[test_case(2, include_str!("info/test_data/listxml_coco.xml"), "coco2b", None, &[("ext", Some("multi"))], &[("ext", Some("multi"))])]
	fn changed_slots(
		_index: usize,
		info_xml: &str,
		machine_name: &str,
		opts1: Option<&[(&str, Option<&str>)]>,
		opts2: &[(&str, Option<&str>)],
		expected: &[(&str, Option<&str>)],
	) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| false)
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		// create the original config
		let machine_index = info_db.machines().find_index(machine_name).unwrap();
		let original_config = MachineConfig::new(info_db, machine_index);

		// create the config for `opts1` (if `Some`)
		let config1 = opts1.map(|opts1| {
			let mut config = original_config.clone();
			for (slot, opt) in opts1 {
				config = config.set_slot_option(slot, *opt).unwrap().unwrap();
			}
			config
		});

		// create the config for `opts2`
		let config2 = {
			let mut config = original_config;
			for (slot, opt) in opts2 {
				config = config.set_slot_option(slot, *opt).unwrap().unwrap();
			}
			config
		};

		// get the changed slots
		let actual = config2.changed_slots(config1.as_ref());

		// and validate the changes
		let expected = expected
			.iter()
			.map(|(slot, opt)| (slot.to_string(), opt.map(|x| x.to_string())))
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
