//! General utility functions for parsing stuff outputted by MAME
use anyhow::Error;
use anyhow::Result;

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

#[cfg(test)]
mod test {
	use test_case::test_case;

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
}
