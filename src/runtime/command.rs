use std::borrow::Cow;

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
		}
	}
}

fn bool_str(b: bool) -> &'static str {
	if b {
		"true"
	} else {
		"false"
	}
}

fn pairs_command_text(base: &[&str], args: &[(&str, &str)]) -> Cow<'static, str> {
	base.iter()
		.copied()
		.map(Cow::Borrowed)
		.chain(args.iter().flat_map(|(name, value)| {
			let name = Cow::Borrowed(*name);
			let value = if value.contains(' ') {
				Cow::Owned(format!("\"{}\"", value))
			} else {
				Cow::Borrowed(*value)
			};
			[name, value]
		}))
		.join(" ")
		.into()
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::MameCommand;

	#[test_case(0, MameCommand::Stop, "STOP")]
	#[test_case(1, MameCommand::Start { machine_name: "coco2b", initial_loads: &[("ext:fdc:wd17xx:0", "foo.dsk")]}, "START coco2b ext:fdc:wd17xx:0 foo.dsk")]
	#[test_case(2, MameCommand::LoadImage(&[("ext:fdc:wd17xx:0", "foo bar.dsk")]), "LOAD ext:fdc:wd17xx:0 \"foo bar.dsk\"")]
	fn command_test(_index: usize, command: MameCommand<'_>, expected: &str) {
		let actual = command.text();
		assert_eq!(expected, actual);
	}
}
