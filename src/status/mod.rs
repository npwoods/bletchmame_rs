mod parse;
mod validate;

use std::borrow::Cow;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::io::BufRead;
use std::sync::Arc;

use anyhow::Result;
use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;
use strum::EnumProperty;
use strum::EnumString;
use tracing::debug;

use crate::debugstr::DebugString;
use crate::info::InfoDb;
use crate::status::parse::parse_update;
use crate::status::validate::validate_status;
use crate::version::MameVersion;

#[derive(Clone)]
pub struct Status {
	pub running: Option<Running>,
	pub build: MameVersion,
}

impl Status {
	pub fn new(old_status: Option<&Self>, update: Update) -> Self {
		let running: Option<Running> = update.running.map(|running| {
			let status_running = old_status
				.and_then(|x| x.running.as_ref())
				.map(Cow::Borrowed)
				.unwrap_or_else(|| Cow::Owned(Running::default()));
			let mut status_running_images = status_running.images.iter().collect::<Vec<_>>();

			let machine_name = running.machine_name;
			let is_paused = running.is_paused.unwrap_or(status_running.is_paused);
			let is_throttled = running.is_throttled.unwrap_or(status_running.is_throttled);
			let throttle_rate = running.throttle_rate.unwrap_or(status_running.throttle_rate);
			let system_mute = running.system_mute.or(status_running.system_mute);
			let sound_attenuation = running.sound_attenuation.or(status_running.sound_attenuation);
			let is_recording = running.is_recording.unwrap_or(status_running.is_recording);

			let images = running.images.map(|images| {
				images
					.into_iter()
					.filter_map(|update_image| {
						let details = if let Some(details) = update_image.details {
							details
						} else {
							let idx = status_running_images.iter().position(|x| x.tag == update_image.tag)?;
							status_running_images.remove(idx).details.clone()
						};

						let new_status_image = Image {
							tag: update_image.tag,
							filename: update_image.filename,
							details,
						};
						Some(new_status_image)
					})
					.collect()
			});
			let images = collect_or_clone_existing(images, &status_running.images);
			let slots = collect_or_clone_existing(running.slots, &status_running.slots);
			let inputs = collect_or_clone_existing(running.inputs, &status_running.inputs);
			let input_device_classes =
				collect_or_clone_existing(running.input_device_classes, &status_running.input_device_classes);

			Running {
				machine_name,
				is_paused,
				is_throttled,
				throttle_rate,
				system_mute,
				sound_attenuation,
				is_recording,
				images,
				slots,
				inputs,
				input_device_classes,
			}
		});
		debug!(running=?running, "Status::merge()");
		Self {
			running,
			build: update.build,
		}
	}

	pub fn validate(&self, info_db: &InfoDb) -> std::result::Result<(), ValidationError> {
		validate_status(self, info_db)
	}
}

impl Debug for Status {
	fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
		fmt.debug_struct("Status")
			.field("running", &self.running.as_ref().map(DebugString::elipsis))
			.field("build", &self.build)
			.finish()
	}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Running {
	pub machine_name: String,
	pub is_paused: bool,
	pub is_throttled: bool,
	pub throttle_rate: f32,
	pub system_mute: Option<bool>,
	pub sound_attenuation: Option<i32>,
	pub is_recording: bool,
	pub images: Arc<[Image]>,
	pub slots: Arc<[Slot]>,
	pub inputs: Arc<[Input]>,
	pub input_device_classes: Arc<[InputDeviceClass]>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Image {
	pub tag: String,
	pub filename: Option<String>,
	pub details: ImageDetails,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct ImageDetails {
	pub instance_name: String,
	pub is_readable: bool,
	pub is_writeable: bool,
	pub is_creatable: bool,
	pub must_be_loaded: bool,
	pub formats: Arc<[ImageFormat]>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Hash, Eq)]
pub struct ImageFormat {
	pub name: String,
	pub description: String,
	pub extensions: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
pub struct Update {
	running: Option<RunningUpdate>,
	build: MameVersion,
}

impl Update {
	pub fn parse(reader: impl BufRead) -> Result<Self> {
		parse_update(reader)
	}

	pub fn validate(&self, info_db: &InfoDb) -> std::result::Result<(), ValidationError> {
		Status::new(None, self.clone()).validate(info_db)
	}

	pub fn is_running(&self) -> bool {
		self.running.is_some()
	}
}

impl Debug for Update {
	fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
		fmt.debug_struct("Update")
			.field("running", &self.running.as_ref().map(DebugString::elipsis))
			.field("build", &self.build)
			.finish()
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
struct RunningUpdate {
	pub machine_name: String,
	pub is_paused: Option<bool>,
	pub is_throttled: Option<bool>,
	pub throttle_rate: Option<f32>,
	pub system_mute: Option<bool>,
	pub sound_attenuation: Option<i32>,
	pub is_recording: Option<bool>,
	pub images: Option<Vec<ImageUpdate>>,
	pub slots: Option<Vec<Slot>>,
	pub inputs: Option<Vec<Input>>,
	pub input_device_classes: Option<Vec<InputDeviceClass>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
struct ImageUpdate {
	pub tag: String,
	pub filename: Option<String>,
	pub details: Option<ImageDetails>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Slot {
	pub name: String,
	pub fixed: bool,
	pub has_selectable_options: bool,
	pub options: Vec<SlotOption>,
	pub current_option: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct SlotOption {
	pub name: String,
	pub selectable: bool,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct Input {
	pub port_tag: Arc<str>,
	pub mask: u32,
	pub class: Option<InputClass>,
	pub group: u8,
	pub input_type: u32,
	pub player: u8,
	pub is_analog: bool,
	pub name: String,
	pub first_keyboard_code: Option<u32>,
	pub seq_standard_tokens: Option<String>,
	pub seq_increment_tokens: Option<String>,
	pub seq_decrement_tokens: Option<String>,
}

impl Debug for Input {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		let port = format!("{} {:#x}", self.port_tag, self.mask);
		let seq_tokens_all = [
			("", &self.seq_standard_tokens),
			("[◄] ", &self.seq_decrement_tokens),
			("[►] ", &self.seq_increment_tokens),
		];
		let seq_tokens = seq_tokens_all
			.iter()
			.filter_map(|(prefix, tokens)| tokens.as_deref().map(|tokens| format!("{prefix}{tokens}")))
			.join(" ; ");

		f.debug_struct("Input")
			.field("port", &port)
			.field("class", &self.class)
			.field("group", &self.group)
			.field("input_type", &self.input_type)
			.field("player", &self.player)
			.field("is_analog", &self.is_analog)
			.field("name", &self.name)
			.field("first_keyboard_code", &self.first_keyboard_code)
			.field("seq_tokens", &seq_tokens)
			.finish()
	}
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, EnumProperty, EnumString)]
#[strum(ascii_case_insensitive)]
pub enum InputClass {
	#[strum(props(Title = "Joysticks and Controllers"))]
	Controller,
	#[strum(props(Title = "Keyboard"))]
	Keyboard,
	#[strum(props(Title = "Miscellaneous Input"))]
	Misc,
	#[strum(props(Title = "Configuration"))]
	Config,
	#[strum(props(Title = "Dip Switches"))]
	DipSwitch,
}

impl InputClass {
	pub fn title(&self) -> &'static str {
		self.get_str("Title").unwrap()
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct InputDeviceClass {
	pub name: InputDeviceClassName,
	pub enabled: bool,
	pub multi: bool,
	pub devices: Vec<InputDevice>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, EnumProperty, EnumString)]
#[strum(ascii_case_insensitive)]
pub enum InputDeviceClassName {
	Keyboard,
	Joystick,
	Lightgun,
	Mouse,
	#[strum(default)]
	Other(String),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct InputDevice {
	pub name: String,
	pub id: String,
	pub devindex: u32,
	pub items: Vec<InputDeviceItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct InputDeviceItem {
	pub name: String,
	pub token: String,
	pub code: String,
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum ValidationError {
	#[error("Version mismatch; MAME is {0} InfoDb is {1}")]
	VersionMismatch(MameVersion, MameVersion),
	#[error("Invalid Update: {0:?}")]
	Invalid(Vec<UpdateXmlProblem>),
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum UpdateXmlProblem {
	#[error("Machine {0} not found in InfoDb")]
	UnknownMachine(String),
}

fn collect_or_clone_existing<T>(update: Option<Vec<T>>, existing: &Arc<[T]>) -> Arc<[T]>
where
	T: PartialEq,
{
	let update = update.and_then(|update| (update.as_slice() != existing.as_ref()).then_some(update));
	if let Some(update) = update {
		update.into_iter().collect()
	} else {
		existing.clone()
	}
}

#[cfg(test)]
mod test {
	use std::io::BufReader;
	use std::str::FromStr;
	use std::sync::Arc;

	use test_case::test_case;

	use crate::status::Status;
	use crate::status::Update;
	use crate::status::parse::parse_update;

	use super::InputDeviceClassName;

	#[test]
	fn session() {
		let xml0 = include_str!("test_data/status_mame0270_1.xml");
		let xml1 = include_str!("test_data/status_mame0270_coco2b_1.xml");
		let xml2 = include_str!("test_data/status_mame0270_coco2b_2.xml");
		let xml3 = include_str!("test_data/status_mame0270_coco2b_3.xml");
		let xml4 = include_str!("test_data/status_mame0270_coco2b_4.xml");

		fn update(xml: &str) -> Update {
			let reader = BufReader::new(xml.as_bytes());
			parse_update(reader).unwrap()
		}

		// status after a non-running update
		let status = Status::new(None, update(xml0));
		assert!(status.running.is_none());

		// status after running
		let status = Status::new(Some(&status), update(xml1));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((true, true, 1.0), actual);

		// unpaused...
		let status = Status::new(Some(&status), update(xml2));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, true, 1.0), actual);

		// null update
		let status = Status::new(Some(&status), update(xml3));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, true, 1.0), actual);

		// speed it up!
		let status = Status::new(Some(&status), update(xml4));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, false, 3.0), actual);

		// and apply the same update again!
		let old_run = run;
		let status = Status::new(Some(&status), update(xml4));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, false, 3.0), actual);
		assert!(Arc::ptr_eq(&old_run.images, &run.images));
		assert!(Arc::ptr_eq(&old_run.slots, &run.slots));
		assert!(Arc::ptr_eq(&old_run.inputs, &run.inputs));
		assert!(Arc::ptr_eq(&old_run.input_device_classes, &run.input_device_classes));
	}

	#[test_case(0, "keyboard", InputDeviceClassName::Keyboard)]
	#[test_case(1, "joystick", InputDeviceClassName::Joystick)]
	#[test_case(2, "lightgun", InputDeviceClassName::Lightgun)]
	#[test_case(3, "mouse", InputDeviceClassName::Mouse)]
	#[test_case(4, "xyz", InputDeviceClassName::Other("xyz".into()))]
	fn parse_input_device_class_name(_index: usize, s: &str, expected: InputDeviceClassName) {
		let actual = InputDeviceClassName::from_str(s).unwrap();
		assert_eq!(expected, actual);
	}
}
