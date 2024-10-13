mod args;
pub mod controller;

#[derive(Debug)]
pub enum MameWindowing {
	Attached(String),
	Windowed,
	#[allow(dead_code)]
	WindowedMaximized,
	#[allow(dead_code)]
	Fullscreen,
}
