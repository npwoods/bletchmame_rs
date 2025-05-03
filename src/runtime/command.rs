use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::Path;

use derive_enum_all_values::AllValues;
use itertools::Itertools;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MameCommand<'a> {
	Start {
		machine_name: &'a str,
		initial_loads: &'a [(&'a str, &'a str)],
	},
	Stop,
	SoftReset,
	HardReset,
	Pause,
	Resume,
	ClassicMenu,
	Throttled(bool),
	ThrottleRate(f32),
	SetAttenuation(i32),
	LoadImage(&'a [(&'a str, &'a str)]),
	UnloadImage(&'a str),
	ChangeSlots(&'a [(&'a str, &'a str)]),
	StateLoad(&'a str),
	StateSave(&'a str),
	SaveSnapshot(u32, &'a str),
	BeginRecording(&'a str, MovieFormat),
	EndRecording,
	Debugger,
}

impl MameCommand<'_> {
	pub fn text(&self) -> Cow<'static, str> {
		match self {
			MameCommand::Start {
				machine_name,
				initial_loads,
			} => pairs_command_text(&["START", machine_name], initial_loads),
			MameCommand::Stop => "STOP".into(),
			MameCommand::SoftReset => "SOFT_RESET".into(),
			MameCommand::HardReset => "HARD_RESET".into(),
			MameCommand::Pause => "PAUSE".into(),
			MameCommand::Resume => "RESUME".into(),
			MameCommand::ClassicMenu => "CLASSIC_MENU".into(),
			MameCommand::Throttled(throttled) => format!("THROTTLED {}", bool_str(*throttled)).into(),
			MameCommand::ThrottleRate(throttle) => format!("THROTTLE_RATE {}", throttle).into(),
			MameCommand::SetAttenuation(attenuation) => format!("SET_ATTENUATION {}", attenuation).into(),
			MameCommand::LoadImage(loads) => pairs_command_text(&["LOAD"], loads),
			MameCommand::UnloadImage(tag) => format!("UNLOAD {}", tag).into(),
			MameCommand::ChangeSlots(changes) => pairs_command_text(&["CHANGE_SLOTS"], changes),
			MameCommand::StateLoad(filename) => format!("STATE_LOAD {filename}").into(),
			MameCommand::StateSave(filename) => format!("STATE_SAVE {filename}").into(),
			MameCommand::SaveSnapshot(screen_number, filename) => {
				let filename = quote_if_needed(filename);
				format!("SAVE_SNAPSHOT {screen_number} {filename}").into()
			}
			MameCommand::BeginRecording(filename, format) => format!("BEGIN_RECORDING {filename} {format}").into(),
			MameCommand::EndRecording => "END_RECORDING".into(),
			MameCommand::Debugger => "DEBUGGER".into(),
		}
	}
}

#[derive(AllValues, Copy, Clone, Debug, Default, PartialEq, strum::Display)]
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

fn pairs_command_text(base: &[&str], args: &[(&str, &str)]) -> Cow<'static, str> {
	base.iter()
		.copied()
		.map(Cow::Borrowed)
		.chain(args.iter().flat_map(|(name, value)| {
			let name = Cow::Borrowed(*name);
			let value = quote_if_needed(value);
			[name, value]
		}))
		.join(" ")
		.into()
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

	#[test_case(0, MameCommand::Stop, "STOP")]
	#[test_case(1, MameCommand::Start { machine_name: "coco2b", initial_loads: &[("-ramsize", "")]}, "START coco2b -ramsize \"\"")]
	#[test_case(2, MameCommand::Start { machine_name: "coco2b", initial_loads: &[("-ramsize", "64k")]}, "START coco2b -ramsize 64k")]
	#[test_case(3, MameCommand::Start { machine_name: "coco2b", initial_loads: &[("ext:fdc:wd17xx:0", "foo.dsk")]}, "START coco2b ext:fdc:wd17xx:0 foo.dsk")]
	#[test_case(4, MameCommand::LoadImage(&[("ext:fdc:wd17xx:0", "foo bar.dsk")]), "LOAD ext:fdc:wd17xx:0 \"foo bar.dsk\"")]
	fn command_test(_index: usize, command: MameCommand<'_>, expected: &str) {
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
