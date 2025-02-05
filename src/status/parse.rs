use std::collections::HashSet;
use std::io::BufRead;
use std::sync::Arc;

use anyhow::Error;
use anyhow::Result;
use tracing::event;
use tracing::Level;

use crate::parse::normalize_tag;
use crate::parse::parse_mame_bool;
use crate::status::ImageDetails;
use crate::status::ImageFormat;
use crate::status::ImageUpdate;
use crate::status::RunningUpdate;
use crate::status::Slot;
use crate::status::SlotOption;
use crate::status::Update;
use crate::version::MameVersion;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;

const LOG: Level = Level::TRACE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	Status,
	StatusImages,
	StatusSlots,
	Image,
	ImageDetails,
	ImageDetailsFormat,
	ImageDetailsFormatExtension,
	Slot,
}

const TEXT_CAPTURE_PHASES: &[Phase] = &[Phase::ImageDetailsFormatExtension];

#[derive(Debug)]
struct State {
	phase_stack: Vec<Phase>,
	build: Option<MameVersion>,
	running: RunningUpdate,
	phase_specific: Option<PhaseSpecificState>,
	all_formats: HashSet<Arc<[ImageFormat]>>,
}

#[derive(Debug)]
enum PhaseSpecificState {
	Formats(Vec<ImageFormat>),
	Slot(Option<String>),
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Missing mandatory attribute {0} when parsing status XML")]
	MissingMandatoryAttribute(&'static str),
}

impl State {
	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> Result<Option<Phase>> {
		let phase = self.phase_stack.last().unwrap_or(&Phase::Root);
		let new_phase = match (phase, evt.name().as_ref()) {
			(Phase::Root, b"status") => {
				let [romname, is_paused, app_build] = evt.find_attributes([b"romname", b"paused", b"app_build"])?;
				let machine_name = romname.unwrap_or_default().to_string();
				let is_paused = is_paused.map(|x| parse_mame_bool(x.as_ref())).transpose()?;
				event!(
					LOG,
					"status State::handle_start(): machine_name={} is_paused={:?}",
					machine_name,
					is_paused
				);

				self.build = app_build.map(MameVersion::from);
				self.running.machine_name = machine_name;
				self.running.is_paused = is_paused;
				Some(Phase::Status)
			}
			(Phase::Status, b"video") => {
				let [throttled, throttle_rate] = evt.find_attributes([b"throttled", b"throttle_rate"])?;
				let throttled = throttled.map(parse_mame_bool).transpose()?;
				let throttle_rate = throttle_rate.map(|x| x.parse::<f32>()).transpose()?;

				event!(
					LOG,
					"status State::handle_start(): throttled={:?} throttle_rate={:?}",
					throttled,
					throttle_rate
				);

				self.running.is_throttled = throttled.or(self.running.is_throttled);
				self.running.throttle_rate = throttle_rate.or(self.running.throttle_rate);
				None
			}
			(Phase::Status, b"sound") => {
				let [attenuation] = evt.find_attributes([b"attenuation"])?;
				let attenuation = attenuation.map(|x| x.parse::<i32>()).transpose()?;
				self.running.sound_attenuation = attenuation.or(self.running.sound_attenuation);
				None
			}
			(Phase::Status, b"images") => {
				self.running.images = Some(Vec::new());
				Some(Phase::StatusImages)
			}
			(Phase::Status, b"slots") => {
				self.running.slots = Some(Vec::new());
				Some(Phase::StatusSlots)
			}
			(Phase::StatusImages, b"image") => {
				let [tag, filename] = evt.find_attributes([b"tag", b"filename"])?;
				let tag = tag.ok_or(ThisError::MissingMandatoryAttribute("tag"))?;
				let tag = normalize_tag(tag).to_string();
				let filename = filename.map(|x| x.into_owned());
				let image = ImageUpdate {
					tag,
					filename,
					details: None,
				};
				self.running.images.as_mut().unwrap().push(image);
				Some(Phase::Image)
			}
			(Phase::Image, b"details") => {
				let [instance_name, is_readable, is_writeable, is_creatable, must_be_loaded] =
					evt.find_attributes([
						b"instance_name",
						b"is_readable",
						b"is_writeable",
						b"is_creatable",
						b"must_be_loaded",
					])?;
				let instance_name = instance_name
					.ok_or(ThisError::MissingMandatoryAttribute("instance_name"))?
					.to_string();
				let is_readable =
					parse_mame_bool(is_readable.ok_or(ThisError::MissingMandatoryAttribute("is_readable"))?)?;
				let is_writeable =
					parse_mame_bool(is_writeable.ok_or(ThisError::MissingMandatoryAttribute("is_writeable"))?)?;
				let is_creatable =
					parse_mame_bool(is_creatable.ok_or(ThisError::MissingMandatoryAttribute("is_creatable"))?)?;
				let must_be_loaded =
					parse_mame_bool(must_be_loaded.ok_or(ThisError::MissingMandatoryAttribute("must_be_loaded"))?)?;

				let details = ImageDetails {
					instance_name,
					is_readable,
					is_writeable,
					is_creatable,
					must_be_loaded,
					formats: Default::default(),
				};
				self.running.images.as_mut().unwrap().last_mut().unwrap().details = Some(details);
				self.phase_specific = Some(PhaseSpecificState::Formats(Vec::with_capacity(32)));
				Some(Phase::ImageDetails)
			}
			(Phase::ImageDetails, b"format") => {
				let [name, description] = evt.find_attributes([b"name", b"description"])?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?.to_string();
				let description = description
					.ok_or(ThisError::MissingMandatoryAttribute("description"))?
					.to_string();

				let format = ImageFormat {
					name,
					description,
					extensions: Vec::with_capacity(16),
				};

				let PhaseSpecificState::Formats(formats) = self.phase_specific.as_mut().unwrap() else {
					unreachable!()
				};
				formats.push(format);
				Some(Phase::ImageDetailsFormat)
			}
			(Phase::ImageDetailsFormat, b"extension") => Some(Phase::ImageDetailsFormatExtension),
			(Phase::StatusSlots, b"slot") => {
				let [name, fixed, has_selectable_options, current_option] =
					evt.find_attributes([b"name", b"fixed", b"has_selectable_options", b"current_option"])?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?;
				let name = normalize_tag(name).to_string();
				let fixed = parse_mame_bool(fixed.ok_or(ThisError::MissingMandatoryAttribute("fixed"))?)?;
				let has_selectable_options = parse_mame_bool(
					has_selectable_options.ok_or(ThisError::MissingMandatoryAttribute("has_selectable_options"))?,
				)?;
				let current_option = current_option.map(|x| x.to_string());
				let slot = Slot {
					name,
					fixed,
					has_selectable_options,
					current_option: None,
					options: Vec::new(),
				};
				self.running.slots.as_mut().unwrap().push(slot);
				self.phase_specific = Some(PhaseSpecificState::Slot(current_option));
				Some(Phase::Slot)
			}
			(Phase::Slot, b"option") => {
				// parse attributes
				let [name, selectable] = evt.find_attributes([b"name", b"selectable"])?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?.into_owned();
				let selectable =
					parse_mame_bool(selectable.ok_or(ThisError::MissingMandatoryAttribute("selectable"))?)?;

				// identify the current slot
				let current_slot = self.running.slots.as_mut().unwrap().last_mut().unwrap();

				// is this the current selection?  if so record it
				let PhaseSpecificState::Slot(phase_specific) = self.phase_specific.as_mut().unwrap() else {
					unimplemented!();
				};
				if phase_specific.take_if(|x| x == &name).is_some() {
					current_slot.current_option = Some(current_slot.options.len());
				}

				// finally add the option
				let option = SlotOption { name, selectable };
				current_slot.options.push(option);
				None
			}

			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(&mut self, text: Option<String>) -> Result<()> {
		match self.phase_stack.last().unwrap_or(&Phase::Root) {
			Phase::ImageDetails => {
				let PhaseSpecificState::Formats(formats) = self.phase_specific.take().unwrap() else {
					unreachable!();
				};
				let formats = Arc::<[ImageFormat]>::from(formats);
				let formats = if self.all_formats.insert(formats.clone()) {
					formats
				} else {
					self.all_formats.get(&formats).unwrap().clone()
				};

				let image = self.running.images.as_mut().unwrap().last_mut().unwrap();
				let details = image.details.as_mut().unwrap();
				details.formats = formats;
			}
			Phase::ImageDetailsFormatExtension => {
				let PhaseSpecificState::Formats(formats) = self.phase_specific.as_mut().unwrap() else {
					unreachable!()
				};
				let extensions = &mut formats.last_mut().unwrap().extensions;
				extensions.push(text.unwrap());
			}
			_ => {}
		};
		Ok(())
	}
}

pub fn parse_update(reader: impl BufRead) -> Result<Update> {
	let mut reader = XmlReader::from_reader(reader, false);
	let mut buf = Vec::with_capacity(1024);
	let mut state = State {
		phase_stack: Vec::with_capacity(32),
		build: None,
		running: RunningUpdate::default(),
		phase_specific: None,
		all_formats: HashSet::new(),
	};

	while let Some(evt) = reader.next(&mut buf).map_err(|e| statusxml_err(&reader, e))? {
		match evt {
			XmlEvent::Start(evt) => {
				let new_phase = state.handle_start(evt).map_err(|e| statusxml_err(&reader, e))?;
				if let Some(new_phase) = new_phase {
					state.phase_stack.push(new_phase);

					if TEXT_CAPTURE_PHASES.contains(&new_phase) {
						reader.start_text_capture();
					}
				} else {
					reader.start_unknown_tag();
				}
			}

			XmlEvent::End(s) => {
				state.handle_end(s).map_err(|e| statusxml_err(&reader, e))?;
				state.phase_stack.pop().unwrap();
			}

			XmlEvent::Null => {} // meh
		}
	}

	let running = (!state.running.machine_name.is_empty()).then_some(state.running);
	let result = Update {
		running,
		build: state.build,
	};
	Ok(result)
}

fn statusxml_err(reader: &XmlReader<impl BufRead>, e: impl Into<Error>) -> Error {
	let message = format!("Error parsing status XML at position {}", reader.buffer_position());
	e.into().context(message)
}

#[cfg(test)]
mod test {
	use std::io::BufReader;

	use assert_matches::assert_matches;
	use test_case::test_case;

	use super::parse_update;

	#[test_case(0, include_str!("test_data/status_mame0226_coco2b_1.xml"))]
	#[test_case(1, include_str!("test_data/status_mame0227_coco2b_1.xml"))]
	#[test_case(2, include_str!("test_data/status_mame0270_1.xml"))]
	#[test_case(3, include_str!("test_data/status_mame0270_coco2b_1.xml"))]
	#[test_case(4, include_str!("test_data/status_mame0270_coco2b_2.xml"))]
	#[test_case(5, include_str!("test_data/status_mame0270_coco2b_3.xml"))]
	#[test_case(6, include_str!("test_data/status_mame0270_coco2b_4.xml"))]
	#[test_case(7, include_str!("test_data/status_mame0270_coco2b_5.xml"))]
	#[test_case(8, include_str!("test_data/status_mame0273_c64_1.xml"))]
	fn general(_index: usize, xml: &str) {
		let reader = BufReader::new(xml.as_bytes());
		let result = parse_update(reader);
		assert_matches!(result, Ok(_));
	}

	#[test_case(0, include_str!("test_data/status_mame0226_coco2b_1.xml"), Some(true), Some(1.0))]
	#[test_case(1, include_str!("test_data/status_mame0227_coco2b_1.xml"), Some(true), Some(1.0))]
	#[test_case(2, include_str!("test_data/status_mame0270_coco2b_1.xml"), Some(true), Some(1.0))]
	#[test_case(3, include_str!("test_data/status_mame0270_coco2b_2.xml"), Some(true), Some(1.0))]
	#[test_case(4, include_str!("test_data/status_mame0270_coco2b_3.xml"), None, None)]
	#[test_case(5, include_str!("test_data/status_mame0270_coco2b_4.xml"), Some(false), Some(3.0))]
	fn throttling(_index: usize, xml: &str, expected_is_throttled: Option<bool>, expected_throttle_rate: Option<f32>) {
		let expected = (expected_is_throttled, expected_throttle_rate);

		let reader = BufReader::new(xml.as_bytes());
		let running = parse_update(reader).unwrap().running.unwrap();
		let actual = (running.is_throttled, running.throttle_rate);
		assert_eq!(expected, actual);
	}
}
