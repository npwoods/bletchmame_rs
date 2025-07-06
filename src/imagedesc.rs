use std::borrow::Cow;
use std::path::Path;
use std::str::FromStr;

use anyhow::Error;
use anyhow::Result;

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum ImageDesc<S> {
	File(S),
	Software(S),
	Socket { hostname: S, port: u16 },
}

impl<S> ImageDesc<S>
where
	S: AsRef<str>,
{
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
				let message = format!("File Not Found: {filename}");
				return Err(Error::msg(message));
			}
		}
		Ok(())
	}
}

impl<'a> ImageDesc<&'a str>
where
	Self: 'a,
{
	fn internal_parse(s: &'a str) -> Option<Self> {
		if s.is_empty() {
			None
		} else if let Some(other) = s.strip_prefix("socket.") {
			let (hostname, port) = other.split_once('.')?;
			let port = u16::from_str(port).ok()?;
			Some(Self::Socket { hostname, port })
		} else if s.chars().any(|c| c == '\\' || c == '/' || c == '.' || c == ':') {
			Some(Self::File(s))
		} else {
			Some(Self::Software(s))
		}
	}
}

impl<'a> TryFrom<&'a str> for ImageDesc<&'a str> {
	type Error = Error;

	fn try_from(value: &'a str) -> Result<Self> {
		Self::internal_parse(value).ok_or_else(move || {
			let message = format!("Cannot parse {value}");
			Error::msg(message)
		})
	}
}
