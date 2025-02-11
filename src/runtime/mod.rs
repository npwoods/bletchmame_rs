pub mod args;
pub mod command;
pub mod session;

use std::rc::Rc;

use strum::EnumString;

#[derive(Clone, Debug)]
pub enum MameWindowing {
	Attached(Rc<str>),
	Windowed,
	#[allow(dead_code)]
	WindowedMaximized,
	#[allow(dead_code)]
	Fullscreen,
}

#[derive(Clone, Copy, Debug, Default, EnumString)]
pub enum MameStderr {
	#[default]
	#[strum(ascii_case_insensitive)]
	Capture,
	#[strum(ascii_case_insensitive)]
	Inherit,
}
