mod parse;
mod validate;
use std::borrow::Cow;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::io::BufRead;
use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tracing::Level;
use tracing::event;

use crate::debugstr::DebugString;
use crate::info::InfoDb;
use crate::status::parse::parse_update;
use crate::status::validate::validate_status;
use crate::version::MameVersion;

const LOG: Level = Level::DEBUG;

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
			let sound_attenuation = running.sound_attenuation.unwrap_or(status_running.sound_attenuation);
			let is_recording = running.is_recording.unwrap_or(status_running.is_recording);
			let images = if let Some(images) = running.images {
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
			} else {
				status_running.images.clone()
			};
			let slots = if let Some(slots) = running.slots {
				slots.into_iter().collect()
			} else {
				status_running.slots.clone()
			};

			Running {
				machine_name,
				is_paused,
				is_throttled,
				throttle_rate,
				sound_attenuation,
				is_recording,
				images,
				slots,
			}
		});
		event!(LOG, running=?running, "Status::merge()");
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
	pub sound_attenuation: i32,
	pub is_recording: bool,
	pub images: Arc<[Image]>,
	pub slots: Arc<[Slot]>,
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
	pub sound_attenuation: Option<i32>,
	pub is_recording: Option<bool>,
	pub images: Option<Vec<ImageUpdate>>,
	pub slots: Option<Vec<Slot>>,
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

#[cfg(test)]
mod test {
	use std::io::BufReader;

	use crate::status::Status;
	use crate::status::Update;
	use crate::status::parse::parse_update;

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
	}
}
