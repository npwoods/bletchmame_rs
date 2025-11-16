use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::io::Read;

use anyhow::Result;
use crc32fast::Hasher as Crc32;
use hex::decode_to_slice;
use hex::encode;
use sha1::Digest;
use sha1::Sha1;

#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub struct AssetHash {
	pub crc: Option<u32>,
	pub sha1: Option<[u8; 20]>,
}

impl AssetHash {
	pub fn from_hex_strings(crc_string: Option<&str>, sha1_string: Option<&str>) -> Result<Self> {
		let crc = crc_string.map(parse_hex).transpose()?.map(u32::from_be_bytes);
		let sha1 = sha1_string.map(parse_hex).transpose()?;
		Ok(Self { crc, sha1 })
	}

	pub fn calculate(mut file: impl Read) -> Result<Self> {
		let mut buffer = [0u8; 8192];

		let mut crc = Crc32::new();
		let mut sha1 = Sha1::new();

		loop {
			let n = file.read(&mut buffer)?;
			if n == 0 {
				break;
			}
			crc.update(&buffer[..n]);
			sha1.update(&buffer[..n]);
		}

		let crc = Some(crc.finalize());
		let sha1 = Some(sha1.finalize().into());
		Ok(Self { crc, sha1 })
	}

	pub fn matches(&self, other: &AssetHash) -> bool {
		self.crc.is_none_or(|x| other.crc.is_none_or(|y| x == y))
			&& self.sha1.is_none_or(|x| other.sha1.is_none_or(|y| x == y))
	}
}

impl Display for AssetHash {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		if let Some(crc) = self.crc {
			write!(f, "CRC({})", encode(crc.to_be_bytes()))?;
		}
		if let Some(sha1) = self.sha1 {
			if self.crc.is_some() {
				write!(f, " ")?;
			}
			write!(f, "SHA1({})", encode(sha1))?;
		}
		if self.crc.is_none() && self.sha1.is_none() {
			write!(f, "<<NONE>>")?;
		}
		Ok(())
	}
}

impl Debug for AssetHash {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		Display::fmt(self, f)
	}
}

fn parse_hex<const N: usize>(s: &str) -> Result<[u8; N]> {
	let mut result = [0u8; N];
	decode_to_slice(s, &mut result)?;
	Ok(result)
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::AssetHash;

	#[test]
	pub fn from_hex_strings() {
		let expected_crc = 0xF2345678;
		let expected_sha1 = [
			0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x00, 0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22,
			0x11, 0xFF,
		];
		let expected = AssetHash {
			crc: Some(expected_crc),
			sha1: Some(expected_sha1),
		};

		let crc_string = "F2345678";
		let sha1_string = "99887766554433221100998877665544332211FF";
		let actual = AssetHash::from_hex_strings(Some(crc_string), Some(sha1_string)).unwrap();
		assert_eq!(expected, actual);
	}

	#[rustfmt::skip]
	#[test_case(0, None, None, "<<NONE>>")]
	#[test_case(1, None, Some("99887766554433221100998877665544332211FF"), "SHA1(99887766554433221100998877665544332211ff)")]
	#[test_case(2, Some("ABC45678"), None, "CRC(abc45678)")]
	#[test_case(3, Some("F2345678"), Some("EDE87766554433221100998877665544332211FF"), "CRC(f2345678) SHA1(ede87766554433221100998877665544332211ff)")]
	pub fn display(_index: usize, crc_string: Option<&str>, sha1_string: Option<&str>, expected: &str) {
		let actual = AssetHash::from_hex_strings(crc_string, sha1_string).unwrap();
		assert_eq!(expected, format!("{actual}"));
		assert_eq!(expected, format!("{actual:?}"));
	}
}
