pub mod args;
pub mod session;

use std::rc::Rc;

use strum::EnumString;

use crate::status::Update;

#[derive(Clone, Debug)]
pub enum MameWindowing {
	Attached(Rc<str>),
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
	LoadImage(&'a [(&'a str, &'a str)]),
	UnloadImage(&'a str),
	ChangeSlots(&'a [(&'a str, &'a str)]),
}

#[derive(Debug)]
pub enum MameEvent {
	SessionEnded,
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
