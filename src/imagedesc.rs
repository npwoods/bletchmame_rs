use std::borrow::Cow;
use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use serde::de::Visitor;
use serde::de::value::MapAccessDeserializer;
use smol_str::SmolStr;
use tracing::warn;

use crate::software::is_valid_software_list_name;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageDesc {
	File(SmolStr),
	Software(SmolStr),
	Socket { hostname: SmolStr, port: u16 },
}

#[derive(thiserror::Error, Debug, PartialEq)]
enum ThisError {
	#[error("invalid empty string")]
	InvalidEmptyString,
	#[error("cannot parse socket string: {0}")]
	CannotParseSocketString(String),
	#[error("invalid host name: {0}")]
	InvalidHostName(SmolStr),
	#[error("cannot parse portnumber")]
	CannotParsePortNumber,
	#[error("file not found: {0}")]
	FileNotFound(String),
	#[error("invalid software name: {0}")]
	InvalidSoftwareName(SmolStr),
}

impl ImageDesc {
	pub fn socket(hostname: impl Into<SmolStr>, port: u16) -> Result<Self> {
		Ok(socket(hostname, port)?)
	}

	pub fn from_mame_image_desc(
		desc: impl Into<SmolStr> + AsRef<str>,
		loaded_through_softlist: Option<bool>,
	) -> Result<Self> {
		Ok(from_mame_image_desc(desc, loaded_through_softlist)?)
	}

	pub fn as_mame_image_desc(&self) -> Cow<'_, str> {
		match self {
			Self::File(filename) => Cow::Borrowed(filename.as_ref()),
			Self::Software(software) => Cow::Borrowed(software.as_ref()),
			Self::Socket { hostname, port } => format!("socket.{}:{}", hostname, *port).into(),
		}
	}

	pub fn validate(&self) -> Result<()> {
		if let Self::File(filename) = self
			&& !Path::new(filename).is_file()
			&& available_ports().iter().all(|p| p.port_name != *filename)
		{
			let error = ThisError::FileNotFound(filename.to_string());
			return Err(error.into());
		}
		Ok(())
	}

	pub fn display_name(&self) -> Cow<'_, str> {
		if let Self::File(filename) = self {
			Path::new(filename).file_name().unwrap_or_default().to_string_lossy()
		} else {
			self.as_mame_image_desc()
		}
	}
}

fn from_mame_image_desc(
	desc: impl Into<SmolStr> + AsRef<str>,
	loaded_through_softlist: Option<bool>,
) -> std::result::Result<ImageDesc, ThisError> {
	// sanity check
	if desc.as_ref().is_empty() {
		return Err(ThisError::InvalidEmptyString);
	}

	// is this a socket identifier?
	if let Some(other) = desc.as_ref().strip_prefix("socket.") {
		let (hostname, port) = other
			.split_once(':')
			.ok_or_else(|| ThisError::CannotParseSocketString(desc.as_ref().to_string()))?;
		let port = u16::from_str(port).map_err(|_| ThisError::CannotParsePortNumber)?;
		socket(hostname, port)
	} else {
		// its not a socket... is it a file image or a software image?
		let is_software = match loaded_through_softlist {
			None => {
				// not foolproof, but meh
				is_valid_software_list_name(desc.as_ref())
			}
			Some(false) => false,
			Some(true) => {
				if !is_valid_software_list_name(desc.as_ref()) {
					return Err(ThisError::InvalidSoftwareName(desc.into()));
				}
				true
			}
		};
		if is_software {
			Ok(ImageDesc::Software(desc.as_ref().into()))
		} else {
			Ok(ImageDesc::File(desc.into()))
		}
	}
}

fn socket(hostname: impl Into<SmolStr>, port: u16) -> Result<ImageDesc, ThisError> {
	let hostname = hostname.into();
	if !hostname_validator::is_valid(&hostname) {
		return Err(ThisError::InvalidHostName(hostname));
	}

	Ok(ImageDesc::Socket { hostname, port })
}

/// Simple wrapper around serialport::available_ports() that logs errors
pub fn available_ports() -> Vec<serialport::SerialPortInfo> {
	serialport::available_ports()
		.inspect_err(|e| warn!("Failed to get available ports: {}", e))
		.unwrap_or_default()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
enum ImageDescAlt {
	File { filename: SmolStr },
	Software { name: SmolStr },
	Socket { hostname: SmolStr, port: u16 },
}

impl Serialize for ImageDesc {
	fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		let alt = match self.clone() {
			ImageDesc::File(filename) => ImageDescAlt::File { filename },
			ImageDesc::Software(name) => ImageDescAlt::Software { name },
			ImageDesc::Socket { hostname, port } => ImageDescAlt::Socket { hostname, port },
		};
		alt.serialize(serializer)
	}
}

impl<'de> Deserialize<'de> for ImageDesc {
	fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
	where
		D: serde::Deserializer<'de>,
	{
		// Try deserializing as a string
		struct StringOrEnumVisitor;

		impl<'de> Visitor<'de> for StringOrEnumVisitor {
			type Value = ImageDesc;

			fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
				formatter.write_str("a tagged enum or a string for special case")
			}

			fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
			where
				E: serde::de::Error,
			{
				Ok(ImageDesc::from_mame_image_desc(v, None).unwrap())
			}

			fn visit_map<A>(self, map: A) -> std::result::Result<Self::Value, A::Error>
			where
				A: serde::de::MapAccess<'de>,
			{
				let result = match ImageDescAlt::deserialize(MapAccessDeserializer::new(map))? {
					ImageDescAlt::File { filename } => ImageDesc::File(filename),
					ImageDescAlt::Software { name } => ImageDesc::Software(name),
					ImageDescAlt::Socket { hostname, port } => ImageDesc::Socket { hostname, port },
				};
				Ok(result)
			}
		}

		deserializer.deserialize_any(StringOrEnumVisitor)
	}
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::ImageDesc;
	use super::ThisError;

	#[test_case(0, ImageDesc::File("/foo/bar.img".into()), "/foo/bar.img")]
	#[test_case(1, ImageDesc::Software("abcde".into()), "abcde")]
	#[test_case(2, ImageDesc::socket("contoso.com", 8888).unwrap(), "socket.contoso.com:8888")]
	fn as_mame_image_desc(_index: usize, image_desc: ImageDesc, expected: &str) {
		let actual = image_desc.as_mame_image_desc();
		assert_eq!(expected, actual.as_ref());
	}

	#[test_case(0, "/foo/bar.img", None, Ok(ImageDesc::File("/foo/bar.img".into())))]
	#[test_case(1, "abcde", None, Ok(ImageDesc::Software("abcde".into())))]
	#[test_case(2, "socket.contoso.com:8888", None, Ok(ImageDesc::socket("contoso.com", 8888).unwrap()))]
	#[test_case(3, "", None, Err(ThisError::InvalidEmptyString))]
	#[test_case(4, "socket.INVALID", None, Err(ThisError::CannotParseSocketString("socket.INVALID".into())))]
	#[test_case(5, "socket.------:8888", None, Err(ThisError::InvalidHostName("------".into())))]
	#[test_case(6, "socket.contoso.com:", None, Err(ThisError::CannotParsePortNumber))]
	#[test_case(7, "socket.contoso.com:INVALID", None, Err(ThisError::CannotParsePortNumber))]
	#[test_case(8, "socket.contoso.com:888888", None, Err(ThisError::CannotParsePortNumber))]
	fn from_mame_image_desc(
		_index: usize,
		s: &str,
		loaded_through_softlist: Option<bool>,
		expected: Result<ImageDesc, ThisError>,
	) {
		let actual = super::from_mame_image_desc(s, loaded_through_softlist);
		assert_eq!(expected, actual);
	}

	#[test_case(0, ImageDesc::File("/foo/bar.img".into()), r#"{"type":"file","filename":"/foo/bar.img"}"#)]
	#[test_case(1, ImageDesc::Software("abcde".into()), r#"{"type":"software","name":"abcde"}"#)]
	#[test_case(2, ImageDesc::socket("contoso.com", 8888).unwrap(), r#"{"type":"socket","hostname":"contoso.com","port":8888}"#)]
	fn serialize(_index: usize, desc: ImageDesc, expected: &str) {
		let actual = serde_json::to_string(&desc).unwrap();
		assert_eq!(expected, &actual);
	}

	#[test_case(0, r#"{"type":"file","filename":"/alpha/bravo.img"}"#, ImageDesc::File("/alpha/bravo.img".into()))]
	#[test_case(1, r#"{"type":"software","name":"fghijk"}"#, ImageDesc::Software("fghijk".into()))]
	#[test_case(2, r#"{"type":"socket","hostname":"litware.com","port":7777}"#, ImageDesc::socket("litware.com", 7777).unwrap())]
	#[test_case(3, "\"/foo/bar.img\"", ImageDesc::File("/foo/bar.img".into()))]
	#[test_case(4, "\"abcde\"", ImageDesc::Software("abcde".into()))]
	#[test_case(5, "\"socket.contoso.com:8888\"", ImageDesc::socket("contoso.com", 8888).unwrap())]
	fn deserialize(_index: usize, json: &str, expected: ImageDesc) {
		let actual = serde_json::from_str(json).unwrap();
		assert_eq!(&expected, &actual);
	}
}
