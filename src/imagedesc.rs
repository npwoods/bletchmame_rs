use std::borrow::Cow;
use std::path::Path;
use std::str::FromStr;

use anyhow::Error;
use anyhow::Result;

use crate::software::is_valid_software_list_name;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum ImageDesc<S> {
	File(S),
	Software(S),
	Socket { hostname: S, port: u16 },
}

#[derive(thiserror::Error, Debug, PartialEq)]
enum ThisError {
	#[error("invalid empty string")]
	InvalidEmptyString,
	#[error("cannot parse socket string")]
	CannotParseSocketString(String),
	#[error("cannot parse portnumber")]
	CannotParsePortNumber,
	#[error("file not found: {0}")]
	FileNotFound(String),
}

impl<S> ImageDesc<S>
where
	S: AsRef<str>,
{
	pub fn socket(hostname: S, port: u16) -> Self {
		Self::Socket { hostname, port }
	}

	pub fn encode(&self) -> Cow<'_, str> {
		match self {
			ImageDesc::File(filename) => Cow::Borrowed(filename.as_ref()),
			ImageDesc::Software(software) => Cow::Borrowed(software.as_ref()),
			ImageDesc::Socket { hostname, port } => format!("socket.{}:{}", hostname.as_ref(), *port).into(),
		}
	}

	pub fn validate(&self) -> Result<()> {
		if let Self::File(filename) = self {
			let filename = filename.as_ref();
			if !Path::new(filename).is_file() {
				let error = ThisError::FileNotFound(filename.to_string());
				return Err(error.into());
			}
		}
		Ok(())
	}
}

impl<'a> ImageDesc<&'a str>
where
	Self: 'a,
{
	fn internal_parse(s: &'a str) -> std::result::Result<Self, ThisError> {
		if s.is_empty() {
			Err(ThisError::InvalidEmptyString)
		} else if let Some(other) = s.strip_prefix("socket.") {
			let (hostname, port) = other
				.split_once(':')
				.ok_or_else(|| ThisError::CannotParseSocketString(s.to_string()))?;
			let port = u16::from_str(port).map_err(|_| ThisError::CannotParsePortNumber)?;
			Ok(Self::Socket { hostname, port })
		} else if is_valid_software_list_name(s) {
			// not foolproof, but meh
			Ok(Self::Software(s))
		} else {
			Ok(Self::File(s))
		}
	}
}

impl<'a> TryFrom<&'a str> for ImageDesc<&'a str> {
	type Error = Error;

	fn try_from(value: &'a str) -> Result<Self> {
		Ok(Self::internal_parse(value)?)
	}
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::ImageDesc;
	use super::ThisError;

	#[test_case(0, ImageDesc::File("/foo/bar.img"), "/foo/bar.img")]
	#[test_case(1, ImageDesc::Software("abcde"), "abcde")]
	#[test_case(2, ImageDesc::socket("contoso.com", 8888), "socket.contoso.com:8888")]
	fn encode(_index: usize, image_desc: ImageDesc<&'static str>, expected: &str) {
		let actual = image_desc.encode();
		assert_eq!(expected, actual.as_ref());
	}

	#[test_case(0, "/foo/bar.img", Ok(ImageDesc::File("/foo/bar.img")))]
	#[test_case(1, "abcde", Ok(ImageDesc::Software("abcde")))]
	#[test_case(2, "socket.contoso.com:8888", Ok(ImageDesc::socket("contoso.com", 8888)))]
	#[test_case(3, "", Err(ThisError::InvalidEmptyString))]
	#[test_case(4, "socket.INVALID", Err(ThisError::CannotParseSocketString("socket.INVALID".into())))]
	#[test_case(5, "socket.contoso.com:", Err(ThisError::CannotParsePortNumber))]
	#[test_case(6, "socket.contoso.com:INVALID", Err(ThisError::CannotParsePortNumber))]
	#[test_case(7, "socket.contoso.com:888888", Err(ThisError::CannotParsePortNumber))]
	fn internal_parse(_index: usize, s: &str, expected: Result<ImageDesc<&'static str>, ThisError>) {
		let actual = ImageDesc::internal_parse(s);
		assert_eq!(expected, actual);
	}
}
