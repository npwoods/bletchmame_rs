use std::borrow::Cow;
use std::iter::once;
use std::rc::Rc;

use crate::imagedesc::ImageDesc;
use crate::info::InfoDb;
use crate::info::Machine;
use crate::info::View;
use crate::mconfig::MachineConfig;
use crate::status::Status;

#[derive(Debug)]
pub struct DevicesImagesConfig {
	info_db: Rc<InfoDb>,
	core: Option<DevicesImagesConfigCore>,
}

#[derive(Debug)]
struct DevicesImagesConfigCore {
	machine_configs: MachineConfigPair,
	entries: Box<[InternalEntry]>,
}

#[derive(Clone, Debug)]
struct MachineConfigPair {
	clean: MachineConfig,
	dirty: Option<MachineConfig>,
}

#[derive(Debug)]
struct InternalEntry {
	tag: String,
	subtag_start: usize,
	indent: usize,
	details: InternalEntryDetails,
}

#[derive(Debug, PartialEq)]
enum InternalEntryDetails {
	Slot { current_option_index: Option<usize> },
	Image { image_desc: Option<ImageDesc> },
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
		image_desc: Option<&'a ImageDesc>,
	},
}

#[derive(Debug)]
pub struct EntryOption<'a> {
	pub name: Option<Cow<'a, str>>,
	pub description: Option<Cow<'a, str>>,
}

impl DevicesImagesConfig {
	pub fn new(info_db: Rc<InfoDb>) -> Self {
		Self::with_machine_name(info_db, None).unwrap()
	}

	pub fn with_machine_name(info_db: Rc<InfoDb>, machine_name: Option<&str>) -> Option<Self> {
		let result = if let Some(machine_name) = machine_name {
			let machine_index = info_db.machines().find_index(machine_name).ok()?;
			let machine_config = MachineConfig::new(info_db.clone(), machine_index);
			Self::from(machine_config)
		} else {
			Self { info_db, core: None }
		};
		Some(result)
	}

	pub fn is_dirty(&self) -> bool {
		self.core
			.as_ref()
			.map(|x| x.machine_configs.dirty.is_some())
			.unwrap_or_default()
	}

	pub fn machine_config(&self) -> Option<&'_ MachineConfig> {
		self.core.as_ref().map(|core| {
			core.machine_configs
				.dirty
				.as_ref()
				.unwrap_or(&core.machine_configs.clean)
		})
	}

	pub fn machine(&self) -> Option<Machine<'_>> {
		self.machine_config().map(|x| x.machine())
	}

	pub fn entry_count(&self) -> usize {
		self.core.as_ref().map(|x| x.entries.len()).unwrap_or_default()
	}

	pub fn entry(&self, index: usize) -> Option<Entry> {
		let core = self.core.as_ref()?;
		let internal_entry = core.entries.get(index)?;

		let details = match &internal_entry.details {
			InternalEntryDetails::Slot { current_option_index } => {
				let (_, slot) = core
					.machine_configs
					.current_config()
					.lookup_slot_tag(&internal_entry.tag)
					.unwrap();

				let none_option = EntryOption {
					name: None,
					description: None,
				};
				let options = once(none_option)
					.chain(slot.options().iter().map(|slot_option| {
						let devmachine = self.info_db.machines().find(slot_option.devname()).unwrap();
						let name = Some(slot_option.name().into());
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
			InternalEntryDetails::Image { image_desc } => {
				let image_desc = image_desc.as_ref();
				EntryDetails::Image { image_desc }
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

	pub fn update_status(&self, status: &Status) -> Self {
		// note that this logic won't error; this is because we expect the InfoDB and Status data
		// to be in harmony; we really need a status validation step
		let info_db = self.info_db.clone();
		let machine_configs = self.core.as_ref().map(|x| &x.machine_configs);
		internal_update_status(info_db, machine_configs, status)
	}

	pub fn set_slot_option(&self, tag: &str, new_option_name: Option<&str>) -> Self {
		let core = self
			.core
			.as_ref()
			.expect("set_slot_option() called with no machine_configs available");
		let machine_config = core.machine_configs.current_config();

		// create a new MachineConfig with this option set
		let new_machine_config = machine_config
			.set_slot_option(tag, new_option_name)
			.unwrap()
			.unwrap_or_else(|| machine_config.clone());

		// and bundle it into a pair
		let machine_configs = MachineConfigPair {
			clean: core.machine_configs.clean.clone(),
			dirty: Some(new_machine_config),
		};

		// scavenge a list of images off of our current state
		let images = core
			.entries
			.iter()
			.filter_map(|entry| {
				if let InternalEntryDetails::Image { image_desc } = &entry.details {
					Some((entry.tag.as_str(), image_desc.as_ref()))
				} else {
					None
				}
			})
			.collect::<Vec<_>>();

		// and build a new DevicesImagesConfig
		diconfig_from_machine_configs_and_images(self.info_db.clone(), machine_configs, images)
	}

	pub fn changed_slots(&self, from_original: bool) -> Vec<(String, Option<String>)> {
		self.core
			.as_ref()
			.and_then(|core| {
				core.machine_configs
					.dirty
					.as_ref()
					.map(|dirty| dirty.changed_slots(from_original.then_some(&core.machine_configs.clean)))
			})
			.unwrap_or_default()
	}

	pub fn identify_changed_rows(&self, other: &Self) -> Option<Vec<usize>> {
		identify_changed_rows(
			self.core.as_ref().map(|x| x.entries.as_ref()).unwrap_or_default(),
			other.core.as_ref().map(|x| x.entries.as_ref()).unwrap_or_default(),
		)
	}

	pub fn images(&self) -> impl Iterator<Item = (&'_ str, Option<&'_ ImageDesc>)> {
		self.core.as_ref().unwrap().entries.iter().filter_map(|entry| {
			let InternalEntryDetails::Image { image_desc } = &entry.details else {
				return None;
			};
			Some((entry.tag.as_str(), image_desc.as_ref()))
		})
	}

	pub fn set_images_from_slots(&self, mut image_func: impl FnMut(&str) -> Option<ImageDesc>) -> Self {
		let info_db = self.info_db.clone();
		let Some(core) = self.core.as_ref() else {
			return Self::new(info_db);
		};

		let machine_config = self.machine_config().unwrap();

		let images = {
			let machines_iter = core.entries.iter().filter_map(|entry| {
				let InternalEntryDetails::Slot { current_option_index } = entry.details else {
					return None;
				};
				let current_option_index = current_option_index?;
				let (_, slot) = machine_config.lookup_slot_tag(&entry.tag).unwrap();
				let slot_option = slot.options().get(current_option_index).unwrap();
				let machine = info_db.machines().find(slot_option.devname()).unwrap();
				let tag = format!("{}:{}:", entry.tag, slot_option.name());
				Some((machine, tag))
			});
			let machines_iter = once((self.machine().unwrap(), "".to_string())).chain(machines_iter);
			let device_tag_iter = machines_iter.flat_map(|(machine, tag)| {
				machine
					.devices()
					.iter()
					.filter(|device| !device.tag().contains(':'))
					.map(|device| format!("{}{}", tag, device.tag()))
					.collect::<Vec<_>>()
			});
			device_tag_iter
				.map(|tag| {
					let image_desc = image_func(&tag);
					(tag, image_desc)
				})
				.collect::<Vec<_>>()
		};
		let images = images
			.iter()
			.map(|(tag, image_desc)| (tag.as_str(), image_desc.as_ref()))
			.collect::<Vec<_>>();

		let machine_configs = core.machine_configs.clone();
		diconfig_from_machine_configs_and_images(info_db, machine_configs, images)
	}

	pub fn reset_to_defaults(&self) -> Self {
		let info_db = self.info_db.clone();
		if let Some(machine_config) = self.machine_config() {
			let machine_index = machine_config.machine().index();
			let machine_config = MachineConfig::from_machine_index(info_db, machine_index);
			Self::from(machine_config)
		} else {
			Self::new(info_db)
		}
	}
}

impl From<MachineConfig> for DevicesImagesConfig {
	fn from(value: MachineConfig) -> Self {
		let info_db = value.info_db.clone();
		let machine_configs = MachineConfigPair {
			clean: value,
			dirty: None,
		};
		diconfig_from_machine_configs_and_images(info_db, machine_configs, [].into())
	}
}

impl MachineConfigPair {
	pub fn current_config(&self) -> &'_ MachineConfig {
		self.dirty.as_ref().unwrap_or(&self.clean)
	}
}

fn internal_update_status(
	info_db: Rc<InfoDb>,
	machine_configs: Option<&MachineConfigPair>,
	status: &Status,
) -> DevicesImagesConfig {
	// basic check; if not running, the config becomes extremely simple
	let Some(running) = status.running.as_ref() else {
		return DevicesImagesConfig::new(info_db);
	};

	// find the machine index identified in the status
	let machine_index = info_db.machines().find_index(&running.machine_name).unwrap();

	// the machine_configs passed in is only relevant if the machine_index matches; if we don't
	// have one we need to create it
	let machine_configs = machine_configs
		.and_then(|cfgs| (cfgs.clean.machine_index == machine_index).then_some(cfgs))
		.cloned()
		.unwrap_or_else(|| MachineConfigPair {
			clean: MachineConfig::new(info_db.clone(), machine_index),
			dirty: None,
		});

	// run the statuses through the clean config; and return a new MachineConfig if anything changed
	let new_machine_config =
		running
			.slots
			.iter()
			.filter(|slot| slot.has_selectable_options)
			.fold(None, |config, slot| {
				let tag = &slot.name;
				let new_option_name = slot.current_option.and_then(|idx| {
					let slot_option = &slot.options[idx];
					slot_option.selectable.then_some(slot_option.name.as_str())
				});
				config
					.as_ref()
					.unwrap_or(&machine_configs.clean)
					.set_slot_option(tag, new_option_name)
					.expect("MachineConfig::set_slot_option() should not error")
					.or(config)
			});

	// if we got a new config, this is the new "clean" and we zap the dirty; otherwise keep what we have
	let machine_configs = new_machine_config
		.map(|new_machine_config| MachineConfigPair {
			clean: new_machine_config,
			dirty: None,
		})
		.unwrap_or(machine_configs);

	// we built the MachineConfigs; now we need to fold images in to get a DevicesImagesConfig
	let images = running
		.images
		.iter()
		.map(|x| (x.tag.as_str(), x.image_desc.as_ref()))
		.collect::<Vec<_>>();
	diconfig_from_machine_configs_and_images(info_db, machine_configs, images)
}

fn diconfig_from_machine_configs_and_images(
	info_db: Rc<InfoDb>,
	machine_configs: MachineConfigPair,
	images: Vec<(&'_ str, Option<&'_ ImageDesc>)>,
) -> DevicesImagesConfig {
	// identify all images
	let mut images = images.into_iter().map(Some).collect::<Vec<_>>();

	// traverse the slot hierarchy
	let mut entries = Vec::new();
	machine_configs
		.current_config()
		.visit_slots(|indent, base_tag, _, slot, current_option_index| {
			// add this slot
			let tag = format!("{}{}", base_tag, slot.name());
			let subtag_start = base_tag.len();
			let details = InternalEntryDetails::Slot { current_option_index };
			let entry = InternalEntry {
				tag,
				subtag_start,
				indent,
				details,
			};
			entries.push(entry);

			// pull any images out and add them
			let tag_prefix = format!("{}{}:", base_tag, slot.name());
			let indent = indent + 1;
			let image_entry_iter = images
				.iter_mut()
				.filter_map(|x| {
					x.take_if(|(tag, _)| tag.starts_with(&tag_prefix) && tag[tag_prefix.len()..].find(':').is_none())
				})
				.map(|(tag, image_desc)| internal_entry_image_from_status(tag, image_desc, &tag_prefix, indent));
			entries.extend(image_entry_iter);
		});

	// and add the remaining images
	let image_entry_iter = images
		.into_iter()
		.flatten()
		.map(|(tag, filename)| internal_entry_image_from_status(tag, filename, "", 0));
	entries.extend(image_entry_iter);

	// sort the results
	entries.sort_by(|a, b| Ord::cmp(&a.tag, &b.tag));
	let entries = entries.into();

	// return the new config
	let core = DevicesImagesConfigCore {
		machine_configs,
		entries,
	};
	let core = Some(core);
	DevicesImagesConfig { info_db, core }
}

fn internal_entry_image_from_status(
	tag: &str,
	image_desc: Option<&ImageDesc>,
	tag_prefix: &str,
	indent: usize,
) -> InternalEntry {
	let tag = tag.to_string();
	let subtag_start = tag_prefix.len();
	let image_desc = image_desc.cloned();
	let details = InternalEntryDetails::Image { image_desc };
	InternalEntry {
		tag,
		subtag_start,
		indent,
		details,
	}
}

fn identify_changed_rows(a: &[InternalEntry], b: &[InternalEntry]) -> Option<Vec<usize>> {
	(a.len() == b.len()).then(|| {
		a.iter()
			.zip(b)
			.enumerate()
			.filter_map(|(index, (a_entry, b_entry))| {
				((a_entry.tag != b_entry.tag) || (a_entry.details != b_entry.details)).then_some(index)
			})
			.collect::<Vec<_>>()
	})
}

#[cfg(test)]
mod test {
	use std::rc::Rc;

	use assert_matches::assert_matches;
	use test_case::test_case;

	use crate::info::InfoDb;
	use crate::info::View;
	use crate::status::Status;
	use crate::status::Update;

	use super::DevicesImagesConfig;

	fn smoke_test_config(config: DevicesImagesConfig) {
		let count = config.entry_count();
		let _ = (0..count).map(|index| config.entry(index).unwrap()).collect::<Vec<_>>();
	}

	#[test_case(0, include_str!("info/test_data/listxml_c64.xml"), include_str!("status/test_data/status_mame0273_c64_1.xml"))]
	#[test_case(1, include_str!("info/test_data/listxml_c64.xml"), include_str!("status/test_data/status_mame0273_c64_epyxfast_1.xml"))]
	#[test_case(2, include_str!("info/test_data/listxml_coco.xml"), include_str!("status/test_data/status_mame0270_coco2b_1.xml"))]
	#[test_case(3, include_str!("info/test_data/listxml_coco.xml"), include_str!("status/test_data/status_mame0270_coco2b_5.xml"))]
	fn update_status(_index: usize, info_xml: &str, status_xml: &str) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| false)
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		// build the status
		let update = Update::parse(status_xml.as_bytes()).unwrap();
		let status = Status::new(None, update);

		// now create the config and update the status
		let config = DevicesImagesConfig::new(info_db);
		let new_config = config.update_status(&status);

		// smoke test!
		smoke_test_config(new_config);
	}

	#[test_case(0, include_str!("info/test_data/listxml_coco.xml"), "coco2b", "ext", Some("multi"))]
	fn set_slot_option(_index: usize, info_xml: &str, machine_name: &str, tag: &str, new_option_name: Option<&str>) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| false)
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		// now create the config and set the option
		let config = DevicesImagesConfig::with_machine_name(info_db, Some(machine_name)).unwrap();
		let new_config = config.set_slot_option(tag, new_option_name);

		// smoke test!
		smoke_test_config(new_config);
	}

	#[test_case(0, include_str!("info/test_data/listxml_alienar.xml"))]
	#[test_case(1, include_str!("info/test_data/listxml_c64.xml"))]
	#[test_case(2, include_str!("info/test_data/listxml_coco.xml"))]
	fn access_all_images(_index: usize, info_xml: &str) {
		// build the InfoDB
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| false)
			.unwrap()
			.unwrap();
		let info_db = Rc::new(info_db);

		for machine in info_db.machines().iter() {
			let diconfig = DevicesImagesConfig::with_machine_name(info_db.clone(), Some(machine.name())).unwrap();
			let diconfig = diconfig.set_images_from_slots(|_| None);
			let machine_config = diconfig.machine_config().unwrap();
			for (tag, _) in diconfig.images() {
				let result = machine_config.lookup_device_tag(tag);
				assert_matches!(
					result,
					Ok(_),
					"failure to lookup device {:?} on machine {:?}",
					tag,
					machine.name()
				);
			}
		}
	}
}
