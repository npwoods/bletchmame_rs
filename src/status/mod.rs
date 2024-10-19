mod parse;

use std::io::BufRead;

use serde::Deserialize;
use serde::Serialize;
use tracing::event;
use tracing::Level;

use crate::status::parse::parse_update;
use crate::Result;

const LOG: Level = Level::TRACE;

#[derive(Debug, Default)]
pub struct Status {
	pub has_initialized: bool,
	pub running: Option<StatusRunning>,
}

impl Status {
	pub fn merge(&mut self, update: Update) {
		let running = update.running.map(|running| {
			let status_running = self.running.take().unwrap_or_default();

			let machine_name = running.machine_name;
			let is_paused = running.is_paused.unwrap_or(status_running.is_paused);
			let is_throttled = running.is_throttled.unwrap_or(status_running.is_throttled);
			let throttle_rate = running.throttle_rate.unwrap_or(status_running.throttle_rate);

			StatusRunning {
				machine_name,
				is_paused,
				is_throttled,
				throttle_rate,
			}
		});
		event!(LOG, "Status::merge(): running={:?}", running);
		self.running = running;
		self.has_initialized = true;
	}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusRunning {
	pub machine_name: String,
	pub is_paused: bool,
	pub is_throttled: bool,
	pub throttle_rate: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Update {
	running: Option<UpdateRunning>,
}

impl Update {
	pub fn parse(reader: impl BufRead) -> Result<Self> {
		parse_update(reader)
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
struct UpdateRunning {
	pub machine_name: String,
	pub is_paused: Option<bool>,
	pub is_throttled: Option<bool>,
	pub throttle_rate: Option<f32>,
}
