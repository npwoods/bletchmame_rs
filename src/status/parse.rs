use std::collections::HashSet;
use std::io::BufRead;
use std::sync::Arc;

use anyhow::Error;
use anyhow::Result;
use tracing::debug;

use crate::parse::normalize_tag;
use crate::parse::parse_mame_bool;
use crate::runtime::command::SeqType;
use crate::status::ImageDetails;
use crate::status::ImageFormat;
use crate::status::ImageUpdate;
use crate::status::Input;
use crate::status::InputDevice;
use crate::status::InputDeviceClass;
use crate::status::InputDeviceItem;
use crate::status::RunningUpdate;
use crate::status::Slot;
use crate::status::SlotOption;
use crate::status::Update;
use crate::version::MameVersion;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	Status,
	StatusImages,
	StatusSlots,
	StatusInputs,
	StatusInputDevices,
	Image,
	ImageDetails,
	ImageDetailsFormat,
	ImageDetailsFormatExtension,
	Slot,
	Input,
	InputDeviceClass,
	InputDevice,
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
				let [romname, is_paused, app_build, app_version] =
					evt.find_attributes([b"romname", b"paused", b"app_build", b"app_version"])?;
				let machine_name = romname.unwrap_or_default().to_string();
				let is_paused = is_paused.map(|x| parse_mame_bool(x.as_ref())).transpose()?;
				debug!(
					machine_name=?machine_name,
					is_paused=?is_paused,
					"status State::handle_start()"
				);

				let app_build = app_build.map(MameVersion::from);
				let app_version = app_version.and_then(MameVersion::parse_simple);

				self.build = app_build.or(app_version);
				self.running.machine_name = machine_name;
				self.running.is_paused = is_paused;
				Some(Phase::Status)
			}
			(Phase::Status, b"video") => {
				let [throttled, throttle_rate, is_recording] =
					evt.find_attributes([b"throttled", b"throttle_rate", b"is_recording"])?;
				let throttled = throttled.map(parse_mame_bool).transpose()?;
				let throttle_rate = throttle_rate.map(|x| x.parse::<f32>()).transpose()?;
				let is_recording = is_recording.map(parse_mame_bool).transpose()?;

				debug!(
					throttled=?throttled,
					throttle_rate=?throttle_rate,
					"status State::handle_start()"
				);

				self.running.is_throttled = throttled.or(self.running.is_throttled);
				self.running.throttle_rate = throttle_rate.or(self.running.throttle_rate);
				self.running.is_recording = is_recording.or(self.running.is_recording);
				None
			}
			(Phase::Status, b"sound") => {
				let [system_mute, attenuation] = evt.find_attributes([b"system_mute", b"attenuation"])?;
				let system_mute = system_mute.map(parse_mame_bool).transpose()?;
				let attenuation = attenuation.map(|x| x.parse::<i32>()).transpose()?;
				self.running.system_mute = system_mute.or(self.running.system_mute);
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
			(Phase::Status, b"inputs") => {
				self.running.inputs = Some(Vec::new());
				Some(Phase::StatusInputs)
			}
			(Phase::Status, b"input_devices") => {
				self.running.input_device_classes = Some(Vec::new());
				Some(Phase::StatusInputDevices)
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
			(Phase::StatusInputs, b"input") => {
				let [
					port_tag,
					mask,
					class,
					group,
					input_type,
					player,
					is_analog,
					name,
					first_keyboard_code,
				] = evt.find_attributes([
					b"port_tag",
					b"mask",
					b"class",
					b"group",
					b"type",
					b"player",
					b"is_analog",
					b"name",
					b"first_keyboard_code",
				])?;

				let port_tag = port_tag.ok_or(ThisError::MissingMandatoryAttribute("port_tag"))?;
				let port_tag = normalize_tag(port_tag).into();
				let mask = mask.ok_or(ThisError::MissingMandatoryAttribute("mask"))?.parse()?;
				let class = class.ok_or(ThisError::MissingMandatoryAttribute("class"))?.parse().ok();
				let group = group.ok_or(ThisError::MissingMandatoryAttribute("group"))?.parse()?;
				let input_type = input_type
					.ok_or(ThisError::MissingMandatoryAttribute("type"))?
					.parse()?;
				let player = player.ok_or(ThisError::MissingMandatoryAttribute("player"))?.parse()?;
				let is_analog = parse_mame_bool(is_analog.ok_or(ThisError::MissingMandatoryAttribute("is_analog"))?)?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?.into_owned();
				let first_keyboard_code = first_keyboard_code.map(|x| x.parse()).transpose()?;

				let input = Input {
					port_tag,
					mask,
					class,
					group,
					input_type,
					player,
					is_analog,
					name,
					first_keyboard_code,
					seq_standard_tokens: None,
					seq_increment_tokens: None,
					seq_decrement_tokens: None,
				};
				self.running.inputs.as_mut().unwrap().push(input);
				Some(Phase::Input)
			}
			(Phase::Input, b"seq") => {
				let [seq_type, tokens] = evt.find_attributes([b"type", b"tokens"])?;
				let seq_type = seq_type.ok_or(ThisError::MissingMandatoryAttribute("seq_type"))?;
				let seq_type = seq_type.parse::<SeqType>()?;
				let tokens = tokens.ok_or(ThisError::MissingMandatoryAttribute("tokens"))?;

				let current_input = self.running.inputs.as_mut().unwrap().last_mut().unwrap();
				let current_input_tokens = match seq_type {
					SeqType::Standard => &mut current_input.seq_standard_tokens,
					SeqType::Increment => &mut current_input.seq_increment_tokens,
					SeqType::Decrement => &mut current_input.seq_decrement_tokens,
				};
				*current_input_tokens = Some(tokens.into());
				None
			}
			(Phase::StatusInputDevices, b"class") => {
				let [name, enabled, multi] = evt.find_attributes([b"name", b"enabled", b"multi"])?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?.parse()?;
				let enabled = parse_mame_bool(&enabled.ok_or(ThisError::MissingMandatoryAttribute("enabled"))?)?;
				let multi = parse_mame_bool(&multi.ok_or(ThisError::MissingMandatoryAttribute("multi"))?)?;

				let input_device_class = InputDeviceClass {
					name,
					enabled,
					multi,
					devices: Vec::new(),
				};

				let input_device_classes = self.running.input_device_classes.as_mut().unwrap();
				input_device_classes.push(input_device_class);
				Some(Phase::InputDeviceClass)
			}
			(Phase::InputDeviceClass, b"device") => {
				let [name, id, devindex] = evt.find_attributes([b"name", b"id", b"devindex"])?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?.into_owned();
				let id = id.ok_or(ThisError::MissingMandatoryAttribute("id"))?.into_owned();
				let devindex = devindex.ok_or(ThisError::MissingMandatoryAttribute("devindex"))?;
				let devindex = devindex.parse()?;

				let input_device = InputDevice {
					name,
					id,
					devindex,
					items: Vec::new(),
				};

				let input_device_classes = self.running.input_device_classes.as_mut().unwrap();
				input_device_classes.last_mut().unwrap().devices.push(input_device);
				Some(Phase::InputDevice)
			}
			(Phase::InputDevice, b"item") => {
				let [name, token, code] = evt.find_attributes([b"name", b"token", b"code"])?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?.into_owned();
				let token = token.ok_or(ThisError::MissingMandatoryAttribute("token"))?.into_owned();
				let code = code.ok_or(ThisError::MissingMandatoryAttribute("code"))?.into_owned();
				let item = InputDeviceItem { name, token, code };

				let input_device_classes = self.running.input_device_classes.as_mut().unwrap();
				let input_device = input_device_classes.last_mut().unwrap().devices.last_mut().unwrap();
				input_device.items.push(item);
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
	let build = state.build.take().ok_or_else(|| {
		let message = "Could not identify build";
		Error::msg(message)
	})?;
	let result = Update { running, build };
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

	use crate::status::InputClass;

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

	#[allow(clippy::too_many_arguments)]
	#[test_case(0, include_str!("test_data/status_mame0226_coco2b_1.xml"), "row0", 128, InputClass::Keyboard, 11, 49, 0, false, 
		"g  G", Some(103), Some("KEYCODE_G"), None, None)]
	#[test_case(1, include_str!("test_data/status_mame0226_coco2b_1.xml"), "joystick_rx", 255, InputClass::Controller, 1, 152, 0, true,
		"Right Joystick X", None, Some("JOYCODE_1_XAXIS"), Some("KEYCODE_6PAD OR JOYCODE_1_XAXIS_RIGHT_SWITCH"), Some("KEYCODE_4PAD OR JOYCODE_1_XAXIS_LEFT_SWITCH"))]
	fn inputs(
		_index: usize,
		xml: &str,
		port_tag: &str,
		mask: u32,
		expected_class: InputClass,
		expected_group: u8,
		expected_type: u32,
		expected_player: u8,
		expected_is_analog: bool,
		expected_name: &str,
		expected_first_keyboard_code: Option<u32>,
		expected_seq_standard_tokens: Option<&str>,
		expected_seq_increment_tokens: Option<&str>,
		expected_seq_decrement_tokens: Option<&str>,
	) {
		let expected = (
			Some(expected_class),
			expected_group,
			expected_type,
			expected_player,
			expected_is_analog,
			expected_name,
			expected_first_keyboard_code,
			expected_seq_standard_tokens,
			expected_seq_increment_tokens,
			expected_seq_decrement_tokens,
		);

		let reader = BufReader::new(xml.as_bytes());
		let running = parse_update(reader).unwrap().running.unwrap();
		let input = running
			.inputs
			.as_deref()
			.unwrap()
			.iter()
			.find(|x| x.port_tag.as_ref() == port_tag && x.mask == mask)
			.expect("could not find specified port_tag and mask");
		let actual = (
			input.class,
			input.group,
			input.input_type,
			input.player,
			input.is_analog,
			input.name.as_ref(),
			input.first_keyboard_code,
			input.seq_standard_tokens.as_deref(),
			input.seq_increment_tokens.as_deref(),
			input.seq_decrement_tokens.as_deref(),
		);

		assert_eq!(expected, actual);
	}
}
