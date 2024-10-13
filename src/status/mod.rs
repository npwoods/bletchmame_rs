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
			let machine_name = running.machine_name;
			let is_paused = running
				.is_paused
				.unwrap_or(self.running.as_ref().map(|x| x.is_paused).unwrap_or_default());
			StatusRunning {
				machine_name,
				is_paused,
			}
		});
		event!(LOG, "Status::merge(): running={:?}", running);
		self.running = running;
		self.has_initialized = true;
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusRunning {
	pub machine_name: String,
	pub is_paused: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Update {
	running: Option<UpdateRunning>,
}

impl Update {
	pub fn parse(reader: impl BufRead) -> Result<Self> {
		parse_update(reader)
	}
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
struct UpdateRunning {
	pub machine_name: String,
	pub is_paused: Option<bool>,
}
