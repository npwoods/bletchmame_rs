mod parse;

use std::fmt::Debug;
use std::fmt::Formatter;
use std::io::BufRead;

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use tracing::event;
use tracing::Level;

use crate::debugstr::DebugString;
use crate::status::parse::parse_update;
use crate::version::MameVersion;

const LOG: Level = Level::TRACE;

#[derive(Default)]
pub struct Status {
	pub has_initialized: bool,
	pub running: Option<StatusRunning>,
	pub build: Option<MameVersion>,
}

impl Status {
	pub fn merge(&mut self, update: Update) {
		let running = update.running.map(|running| {
			let status_running = self.running.take().unwrap_or_default();

			let machine_name = running.machine_name;
			let is_paused = running.is_paused.unwrap_or(status_running.is_paused);
			let is_throttled = running.is_throttled.unwrap_or(status_running.is_throttled);
			let throttle_rate = running.throttle_rate.unwrap_or(status_running.throttle_rate);
			let sound_attenuation = running.sound_attenuation.unwrap_or(status_running.sound_attenuation);

			StatusRunning {
				machine_name,
				is_paused,
				is_throttled,
				throttle_rate,
				sound_attenuation,
			}
		});
		event!(LOG, "Status::merge(): running={:?}", running);
		self.running = running;
		self.has_initialized = true;
		self.build = update.build
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
pub struct StatusRunning {
	pub machine_name: String,
	pub is_paused: bool,
	pub is_throttled: bool,
	pub throttle_rate: f32,
	pub sound_attenuation: i32,
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
pub struct Update {
	running: Option<UpdateRunning>,
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
struct UpdateRunning {
	pub machine_name: String,
	pub is_paused: Option<bool>,
	pub is_throttled: Option<bool>,
	pub throttle_rate: Option<f32>,
	pub sound_attenuation: Option<i32>,
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
		let mut status = Status::default();
		assert!(status.running.is_none());

		// status after a non-running update
		status.merge(update(xml0));
		assert!(status.running.is_none());

		// status after running
		status.merge(update(xml1));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((true, true, 1.0), actual);

		// unpaused...
		status.merge(update(xml2));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, true, 1.0), actual);

		// null update
		status.merge(update(xml3));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, true, 1.0), actual);

		// speed it up!
		status.merge(update(xml4));
		let run = status.running.as_ref().unwrap();
		let actual = (run.is_paused, run.is_throttled, run.throttle_rate);
		assert_eq!((false, false, 3.0), actual);
	}
}
