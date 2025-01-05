//! General utility functions for parsing stuff outputted by MAME
use std::borrow::Cow;

use anyhow::Error;
use anyhow::Result;
use itertools::Itertools;
use itertools::Position;

/// General parsing function for bool string values outputted by MAME
pub fn parse_mame_bool(text: impl AsRef<str>) -> Result<bool> {
	let text = text.as_ref().trim();
	match text {
		"0" | "false" | "no" => Ok(false),
		"1" | "true" | "yes" => Ok(true),
		_ => {
			let message = format!("Cannot parse boolean value {text:?}");
			Err(Error::msg(message))
		}
	}
}

/// Normalizes tags (e.g. - `:ext` ==> `ext`)
pub fn normalize_tag<'a>(tag: impl Into<Cow<'a, str>>) -> Cow<'a, str> {
	let tag = tag.into();
	if let Some(s) = internal_normalize_tag(tag.as_ref()) {
		s.into()
	} else {
		tag
	}
}

fn internal_normalize_tag(tag: &str) -> Option<String> {
	let normalize_iter =
		tag.chars()
			.map(Some)
			.chain([None])
			.tuple_windows()
			.with_position()
			.map(|(pos, (ch, next))| match (pos != Position::Middle, ch, next) {
				(_, Some(':'), Some(':')) => None,
				(true, Some(':'), _) => None,
				_ => ch,
			});
	normalize_iter
		.clone()
		.any(|x| x.is_none())
		.then(|| normalize_iter.flatten().collect::<String>())
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::internal_normalize_tag;

	#[test_case(0, "0", Ok(false))]
	#[test_case(1, "1", Ok(true))]
	#[test_case(2, "false", Ok(false))]
	#[test_case(3, "true", Ok(true))]
	#[test_case(4, "no", Ok(false))]
	#[test_case(5, "yes", Ok(true))]
	#[test_case(6, "", Err(()))]
	#[test_case(7, "zyx", Err(()))]
	fn parse_mame_bool(_index: usize, s: &str, expected: Result<bool, ()>) {
		let actual = super::parse_mame_bool(s).map_err(|_| ());
		assert_eq!(expected, actual);
	}

	#[test_case(0, "ext", None)]
	#[test_case(1, "ext:", Some("ext"))]
	#[test_case(2, ":ext", Some("ext"))]
	#[test_case(3, ":ext:", Some("ext"))]
	#[test_case(4, "ext:fdcv11:wd17xx:0:qd", None)]
	#[test_case(5, ":ext:fdcv11:wd17xx:0:qd", Some("ext:fdcv11:wd17xx:0:qd"))]
	#[test_case(6, "ext:fdcv11:wd17xx:0:qd:", Some("ext:fdcv11:wd17xx:0:qd"))]
	#[test_case(7, "ext:fdcv11::::wd17xx:0:qd", Some("ext:fdcv11:wd17xx:0:qd"))]
	pub fn normalize_tag(_index: usize, tag: &str, expected: Option<&str>) {
		let actual = internal_normalize_tag(tag);
		assert_eq!(expected, actual.as_deref());
	}
}
