use std::borrow::Cow;
use std::ffi::OsStr;
use std::iter::once;
use std::path::Path;

use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;
use strum::EnumIter;
use strum::EnumProperty;
use strum::EnumString;
use strum::IntoStaticStr;
use strum::VariantArray;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MameCommand(Cow<'static, str>);

impl MameCommand {
	pub fn text(&self) -> &'_ str {
		self.0.as_ref()
	}

	pub fn start(
		machine_name: &str,
		ram_size: Option<u64>,
		initial_loads: &[(impl AsRef<str>, impl AsRef<str>)],
	) -> Self {
		let ram_size_string = ram_size.as_ref().map(u64::to_string).unwrap_or_default();
		let ram_size_args = ["-ramsize", ram_size_string.as_str()];
		let ram_size_args = if ram_size.is_some() {
			ram_size_args.as_slice()
		} else {
			Default::default()
		};

		let initial_loads = initial_loads
			.iter()
			.flat_map(|(tag, filename)| [tag.as_ref(), filename.as_ref()]);
		let args = once(machine_name)
			.chain(ram_size_args.iter().copied())
			.chain(initial_loads);
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

	pub fn seq_set_all<S>(
		port_tag: impl AsRef<str>,
		mask: u32,
		standard_tokens: S,
		decrement_tokens: S,
		increment_tokens: S,
	) -> Self
	where
		S: AsRef<str>,
	{
		let port_tag = port_tag.as_ref();
		let seqs = &[
			(port_tag, mask, SeqType::Standard, standard_tokens),
			(port_tag, mask, SeqType::Decrement, decrement_tokens),
			(port_tag, mask, SeqType::Increment, increment_tokens),
		];
		Self::seq_set(seqs)
	}

	pub fn seq_set(seqs: &[(impl AsRef<str>, u32, SeqType, impl AsRef<str>)]) -> Self {
		let args = seqs.iter().flat_map(|(port_tag, mask, seq_type, tokens)| {
			let port_tag = Cow::Borrowed(port_tag.as_ref());
			let mask = Cow::Owned(mask.to_string());
			let seq_type = Cow::Borrowed(seq_type.into());
			let tokens = Cow::Borrowed(tokens.as_ref());
			[port_tag, mask, seq_type, tokens]
		});
		build("SEQ_SET", args)
	}

	pub fn seq_poll_start(port_tag: impl AsRef<str>, mask: u32, seq_type: SeqType, start_seq: impl AsRef<str>) -> Self {
		let port_tag = port_tag.as_ref();
		let mask = mask.to_string();
		let seq_type = seq_type.into();
		let start_seq = start_seq.as_ref();
		build("SEQ_POLL_START", [port_tag, mask.as_str(), seq_type, start_seq])
	}

	pub fn seq_poll_stop() -> Self {
		Self("SEQ_POLL_STOP".into())
	}

	pub fn set_mouse_enabled(enabled: bool) -> Self {
		let enabled = (enabled as u8).to_string();
		build("SET_MOUSE_ENABLED", [enabled])
	}

	pub fn ping() -> Self {
		Self("PING".into())
	}

	pub fn exit() -> Self {
		Self("EXIT".into())
	}
}

/// Internal method to build a `MameCommand`
fn build<'a, T>(command_name: &'a str, args: impl IntoIterator<Item = T>) -> MameCommand
where
	T: Into<Cow<'a, str>>,
{
	let command_name = Cow::Borrowed(command_name);
	let args = args.into_iter().map(|x| x.into());
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

#[derive(
	Clone,
	Copy,
	Debug,
	Serialize,
	Deserialize,
	PartialEq,
	Eq,
	Hash,
	EnumProperty,
	IntoStaticStr,
	EnumString,
	VariantArray,
)]
#[strum(ascii_case_insensitive)]
#[strum(serialize_all = "lowercase")]
pub enum SeqType {
	#[strum(props(Suffix = ""))]
	Standard,
	#[strum(props(Suffix = " Dec"))]
	Decrement,
	#[strum(props(Suffix = " Inc"))]
	Increment,
}

impl SeqType {
	pub fn suffix(&self) -> &'static str {
		self.get_str("Suffix").unwrap()
	}
}

fn bool_str(b: bool) -> &'static str {
	if b { "true" } else { "false" }
}

fn quote_if_needed(s: Cow<'_, str>) -> Cow<'_, str> {
	if s.is_empty() || s.contains(' ') {
		Cow::Owned(format!("\"{s}\""))
	} else {
		s
	}
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use crate::runtime::command::SeqType;

	use super::MameCommand;

	const EMPTY: &[(&str, &str)] = &[];

	#[rustfmt::skip]
	#[test_case(0, MameCommand::stop(), "STOP")]
	#[test_case(1, MameCommand::start("coco2b", None, EMPTY), "START coco2b")]
	#[test_case(2, MameCommand::start("coco2b", Some(0x10000), EMPTY), "START coco2b -ramsize 65536")]
	#[test_case(3, MameCommand::start("coco2b", None, &[("ext:fdc:wd17xx:0", "foo.dsk")]), "START coco2b ext:fdc:wd17xx:0 foo.dsk")]
	#[test_case(4, MameCommand::load_image("ext:fdc:wd17xx:0", "foo bar.dsk"), "LOAD ext:fdc:wd17xx:0 \"foo bar.dsk\"")]
	#[test_case(5, MameCommand::load_images(&[("ext:fdc:wd17xx:0", "foo bar.dsk")]), "LOAD ext:fdc:wd17xx:0 \"foo bar.dsk\"")]
	#[test_case(6, MameCommand::seq_set(&[("foobar", 0x20, SeqType::Standard, "KEYCODE_X or KEYCODE_Y")]), "SEQ_SET foobar 32 standard \"KEYCODE_X or KEYCODE_Y\"")]
	fn command_test(_index: usize, command: MameCommand, expected: &str) {
		let actual = command.text();
		assert_eq!(expected, actual);
	}

	#[test_case(0, "", "\"\"")]
	#[test_case(1, "Foo", "Foo")]
	#[test_case(2, "Foo Bar", "\"Foo Bar\"")]
	fn quote_if_needed(_index: usize, s: &str, expected: &str) {
		let actual = super::quote_if_needed(s.into());
		assert_eq!(expected, &actual);
	}
}
