mod args;
pub mod controller;
mod session;

use anyhow::Error;
use strum::EnumString;

use crate::status::Update;

#[derive(Debug)]
pub enum MameWindowing {
	Attached(String),
	Windowed,
	#[allow(dead_code)]
	WindowedMaximized,
	#[allow(dead_code)]
	Fullscreen,
}

#[derive(Debug, PartialEq)]
pub enum MameCommand<'a> {
	Exit,
	Start {
		machine_name: &'a str,
		initial_loads: &'a [(&'a str, &'a str)],
	},
	Stop,
	SoftReset,
	HardReset,
	Pause,
	Resume,
	Ping,
	ClassicMenu,
	Throttled(bool),
	ThrottleRate(f32),
	SetAttenuation(i32),
}

#[derive(Debug)]
pub enum MameEvent {
	SessionStarted,
	SessionEnded,
	Error(Error),
	StatusUpdate(Update),
}

#[derive(Clone, Copy, Debug, Default, EnumString)]
pub enum MameStderr {
	#[default]
	#[strum(ascii_case_insensitive)]
	Capture,
	#[strum(ascii_case_insensitive)]
	Inherit,
}
