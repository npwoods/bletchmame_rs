mod parse;

use std::borrow::Cow;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::io::BufRead;
use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tracing::event;
use tracing::Level;

use crate::debugstr::DebugString;
use crate::status::parse::parse_update;
use crate::version::MameVersion;

const LOG: Level = Level::TRACE;

#[derive(Clone, Default)]
pub struct Status {
	pub has_initialized: bool,
	pub running: Option<Running>,
	pub build: Option<MameVersion>,
}

impl Status {
	pub fn merge(&self, update: Update) -> Self {
		let running = update.running.map(|running| {
			let status_running = self
				.running
				.as_ref()
				.map(Cow::Borrowed)
				.unwrap_or_else(|| Cow::Owned(Running::default()));
			let mut status_running_images = status_running.images.iter().collect::<Vec<_>>();

			let machine_name = running.machine_name;
			let is_paused = running.is_paused.unwrap_or(status_running.is_paused);
			let is_throttled = running.is_throttled.unwrap_or(status_running.is_throttled);
			let throttle_rate = running.throttle_rate.unwrap_or(status_running.throttle_rate);
			let sound_attenuation = running.sound_attenuation.unwrap_or(status_running.sound_attenuation);
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
				images,
				slots,
			}
		});
		event!(LOG, "Status::merge(): running={:?}", running);
		Self {
			running,
			has_initialized: true,
			build: update.build,
		}
	}
}

impl Debug for Status {
	fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
		fmt.debug_struct("Status")
			.field("has_initialized", &self.has_initialized)
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
	build: Option<MameVersion>,
}

impl Update {
	pub fn parse(reader: impl BufRead) -> Result<Self> {
		parse_update(reader)
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

#[cfg(test)]
mod test {
	use std::io::BufReader;

	use crate::status::parse::parse_update;
	use crate::status::Status;
	use crate::status::Update;

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

		// fresh status
		let status = Status::default();
		assert!(status.running.is_none());

		// status after a non-running update
		status.merge(update(xml0));
		assert!(status.running.is_none());

		// status after running
		let status = status.merge(update(xml1));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((true, true, 1.0), actual);

		// unpaused...
		let status = status.merge(update(xml2));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, true, 1.0), actual);

		// null update
		let status = status.merge(update(xml3));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, true, 1.0), actual);

		// speed it up!
		let status = status.merge(update(xml4));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, false, 3.0), actual);
	}
}
