use anyhow::Error;
use anyhow::Result;

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("invalid hex string length for '{s}': expected {expected_len} characters")]
	InvalidHexStringLength { s: String, expected_len: usize },
	#[error("invalid hex string '{s}'; non-hex characters at position {position}")]
	InvalidHexString {
		s: String,
		position: usize,
		#[source]
		error: Error,
	},
}

#[derive(Copy, Clone, Debug, Default)]
pub struct AssetHash {
	pub crc: Option<[u8; 4]>,
	pub sha1: Option<[u8; 20]>,
}

impl AssetHash {
	pub fn from_hex_strings(crc: Option<&str>, sha1: Option<&str>) -> Result<Self> {
		let crc = crc.map(parse_hex).transpose()?;
		let sha1 = sha1.map(parse_hex).transpose()?;
		Ok(Self { crc, sha1 })
	}
}

fn parse_hex<const N: usize>(s: &str) -> Result<[u8; N]> {
	if s.len() != N * 2 {
		let error = ThisError::InvalidHexStringLength {
			s: s.to_string(),
			expected_len: N * 2,
		};
		return Err(error.into());
	}

	let result = (0..s.len())
		.step_by(2)
		.map(|position| {
			let src = &s[position..position + 2];
			u8::from_str_radix(src, 16).map_err(|e| {
				let s = s.to_string();
				let error = e.into();
				ThisError::InvalidHexString { s, position, error }
			})
		})
		.collect::<Result<Vec<_>, _>>()?
		.try_into()
		.unwrap();
	Ok(result)
}
