use std::borrow::Cow;
use std::ffi::OsStr;
use std::iter::once;
use std::path::Path;

use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;
use strum::EnumIter;
use strum::IntoStaticStr;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MameCommand(Cow<'static, str>);

impl MameCommand {
	pub fn text(&self) -> &'_ str {
		self.0.as_ref()
	}

	pub fn start(machine_name: &str, initial_loads: &[(impl AsRef<str>, impl AsRef<str>)]) -> Self {
		let initial_loads = initial_loads
			.iter()
			.flat_map(|(tag, filename)| [tag.as_ref(), filename.as_ref()]);
		let args = once(machine_name).chain(initial_loads);
		build("START", args)
	}

	pub fn stop() -> Self {
		Self("STOP".into())
	}

	pub fn soft_reset() -> Self {
		Self("SOFT_RESET".into())
	}

	pub fn hard_reset() -> Self {
		Self("HARD_RESET".into())
	}

	pub fn pause() -> Self {
		Self("PAUSE".into())
	}

	pub fn resume() -> Self {
		Self("RESUME".into())
	}

	pub fn classic_menu() -> Self {
		Self("CLASSIC_MENU".into())
	}

	pub fn throttled(throttled: bool) -> Self {
		build("THROTTLED", [bool_str(throttled)])
	}

	pub fn throttle_rate(rate: f32) -> Self {
		let rate = rate.to_string();
		build("THROTTLE_RATE", [rate.as_str()])
	}

	pub fn set_system_mute(system_mute: bool) -> Self {
		build("SET_SYSTEM_MUTE", [bool_str(system_mute)])
	}

	pub fn set_attenuation(attenuation: i32) -> Self {
		let attenuation = attenuation.to_string();
		build("SET_ATTENUATION", [attenuation.as_str()])
	}

	pub fn load_image(tag: impl AsRef<str>, filename: impl AsRef<str>) -> Self {
		Self::load_images(&[(tag, filename)])
	}

	pub fn load_images(loads: &[(impl AsRef<str>, impl AsRef<str>)]) -> Self {
		let args = loads
			.iter()
			.flat_map(|(tag, filename)| [tag.as_ref(), filename.as_ref()]);
		build("LOAD", args)
	}

	pub fn unload_image(tag: impl AsRef<str>) -> Self {
		let tag = tag.as_ref();
		build("UNLOAD", [tag])
	}

	pub fn change_slots(changes: &[(impl AsRef<str>, Option<impl AsRef<str>>)]) -> Self {
		let args = changes
			.iter()
			.flat_map(|(tag, filename)| [tag.as_ref(), filename.as_ref().map(|x| x.as_ref()).unwrap_or_default()]);
		build("CHANGE_SLOTS", args)
	}

	pub fn state_load(filename: impl AsRef<str>) -> Self {
		let filename = filename.as_ref();
		build("STATE_LOAD", [filename])
	}

	pub fn state_save(filename: impl AsRef<str>) -> Self {
		let filename = filename.as_ref();
		build("STATE_SAVE", [filename])
	}

	pub fn save_snapshot(screen_number: u32, filename: impl AsRef<str>) -> Self {
		let screen_number = screen_number.to_string();
		let filename = filename.as_ref();
		build("SAVE_SNAPSHOT", [screen_number.as_str(), filename])
	}

	pub fn begin_recording(filename: impl AsRef<str>, format: MovieFormat) -> Self {
		let filename = filename.as_ref();
		let format: &'static str = format.into();
		build("BEGIN_RECORDING", [filename, format])
	}

	pub fn end_recording() -> Self {
		Self("END_RECORDING".into())
	}

	pub fn debugger() -> Self {
		Self("DEBUGGER".into())
	}

	pub fn ping() -> Self {
		Self("PING".into())
	}

	pub fn exit() -> Self {
		Self("EXIT".into())
	}
}

/// Internal method to build a `MameCommand`
fn build<'a>(command_name: &'a str, args: impl IntoIterator<Item = &'a str>) -> MameCommand {
	MameCommand(once(command_name).chain(args).map(quote_if_needed).join(" ").into())
}

#[derive(EnumIter, Copy, Clone, Debug, Default, PartialEq, strum::Display, IntoStaticStr)]
pub enum MovieFormat {
	#[default]
	#[strum(to_string = "avi")]
	Avi,
	#[strum(to_string = "mng")]
	Mng,
}

impl TryFrom<&Path> for MovieFormat {
	type Error = ();

	fn try_from(value: &Path) -> Result<Self, Self::Error> {
		match value.extension().and_then(OsStr::to_str) {
			Some("avi") => Ok(Self::Avi),
			Some("mng") => Ok(Self::Mng),
			_ => Err(()),
		}
	}
}

fn bool_str(b: bool) -> &'static str {
	if b { "true" } else { "false" }
}

fn quote_if_needed(s: &str) -> Cow<'_, str> {
	if s.is_empty() || s.contains(' ') {
		Cow::Owned(format!("\"{s}\""))
	} else {
		Cow::Borrowed(s)
	}
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::MameCommand;

	#[rustfmt::skip]
	#[test_case(0, MameCommand::stop(), "STOP")]
	#[test_case(1, MameCommand::start("coco2b", &[("-ramsize", "")]), "START coco2b -ramsize \"\"")]
	#[test_case(2, MameCommand::start("coco2b", &[("-ramsize", "64k")]), "START coco2b -ramsize 64k")]
	#[test_case(3, MameCommand::start("coco2b", &[("ext:fdc:wd17xx:0", "foo.dsk")]), "START coco2b ext:fdc:wd17xx:0 foo.dsk")]
	#[test_case(4, MameCommand::load_image("ext:fdc:wd17xx:0", "foo bar.dsk"), "LOAD ext:fdc:wd17xx:0 \"foo bar.dsk\"")]
	#[test_case(5, MameCommand::load_images(&[("ext:fdc:wd17xx:0", "foo bar.dsk")]), "LOAD ext:fdc:wd17xx:0 \"foo bar.dsk\"")]
	fn command_test(_index: usize, command: MameCommand, expected: &str) {
		let actual = command.text();
		assert_eq!(expected, actual);
	}

	#[test_case(0, "", "\"\"")]
	#[test_case(1, "Foo", "Foo")]
	#[test_case(2, "Foo Bar", "\"Foo Bar\"")]
	fn quote_if_needed(_index: usize, s: &str, expected: &str) {
		let actual = super::quote_if_needed(s);
		assert_eq!(expected, &actual);
	}
}
