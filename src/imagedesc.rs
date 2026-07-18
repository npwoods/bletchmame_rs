use std::borrow::Cow;
use std::path::Path;
use std::str::FromStr;

use anyhow::Error;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use serde::de::Error as _;
use serde::de::Visitor;
use serde::de::value::MapAccessDeserializer;
use smol_str::SmolStr;
use tracing::warn;

use crate::info::Machine;
use crate::info::View;
use crate::software::Software;
use crate::software::is_valid_software_list_name;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageDesc {
	File(SmolStr),
	Software {
		list: Option<SmolStr>,
		name: SmolStr,
		part: Option<SmolStr>,
	},
	Socket {
		hostname: SmolStr,
		port: u16,
	},
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
	#[error("invalid software name: {0}")]
	InvalidSoftwareName(SmolStr),
	#[error("cannot find interface: {0}")]
	CannotFindInterface(SmolStr),
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

	pub fn from_software(
		machine: &Machine,
		software_list: Option<&str>,
		software: &Software,
	) -> Result<Vec<(SmolStr, Self)>> {
		software
			.parts
			.iter()
			.map(|part| {
				machine
					.devices()
					.iter()
					.find(|dev| dev.interfaces().any(|x| x == part.interface))
					.map(|dev| {
						let desc = ImageDesc::Software {
							list: software_list.map(|x| x.into()),
							name: software.name.clone(),
							part: Some(part.name.clone()),
						};
						(dev.tag().into(), desc)
					})
					.ok_or_else(|| ThisError::CannotFindInterface(part.interface.clone()).into())
			})
			.collect::<Result<Vec<_>>>()
	}

	pub fn as_mame_image_desc(&self) -> Cow<'_, str> {
		match self {
			Self::File(filename) => Cow::Borrowed(filename.as_ref()),
			Self::Software {
				list,
				name: software,
				part,
			} => match (list.as_deref(), part.as_deref()) {
				(None, None) => Cow::Borrowed(software.as_ref()),
				(Some(list), None) => format!("{}:{}", list, software).into(),
				(list, Some(part)) => format!("{}:{}:{}", list.unwrap_or_default(), software, part).into(),
			},
			Self::Socket { hostname, port } => format!("socket.{}:{}", hostname, *port).into(),
		}
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
			from_software_desc(desc.as_ref())
		} else {
			Ok(ImageDesc::File(desc.into()))
		}
	}
}

fn from_software_desc(desc: &str) -> std::result::Result<ImageDesc, ThisError> {
	let words = desc.split(':').collect::<Vec<_>>();
	let (list, name, part) = match words.len() {
		1 => Ok((None, words[0], None)),
		2 => Ok((Some(words[0]), words[1], None)),
		3 => Ok((Some(words[0]), words[1], Some(words[2]))),
		_ => Err(ThisError::InvalidSoftwareName(desc.into())),
	}?;
	let list = list.map(|x| x.into());
	let name = name.into();
	let part = part.map(|x| x.into());
	Ok(ImageDesc::Software { list, name, part })
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
enum ImageDescJson {
	File {
		filename: SmolStr,
	},
	Software {
		#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
		desc: Option<SmolStr>,
		#[serde(default, skip_serializing_if = "default_ext::DefaultExt::is_default")]
		name: Option<SmolStr>,
	},
	Socket {
		hostname: SmolStr,
		port: u16,
	},
}

impl From<ImageDesc> for ImageDescJson {
	fn from(value: ImageDesc) -> Self {
		match value {
			ImageDesc::File(filename) => Self::File { filename },
			ImageDesc::Software { .. } => {
				let desc = Some(value.as_mame_image_desc().into());
				Self::Software { desc, name: None }
			}
			ImageDesc::Socket { hostname, port } => Self::Socket { hostname, port },
		}
	}
}

impl TryFrom<ImageDescJson> for ImageDesc {
	type Error = Error;

	fn try_from(value: ImageDescJson) -> Result<Self> {
		let result = match value {
			ImageDescJson::File { filename } => Self::File(filename),
			ImageDescJson::Software { desc, name } => {
				let desc = desc.or(name).unwrap_or_default();
				from_software_desc(&desc)?
			}
			ImageDescJson::Socket { hostname, port } => Self::Socket { hostname, port },
		};
		Ok(result)
	}
}

impl Serialize for ImageDesc {
	fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		ImageDescJson::from(self.clone()).serialize(serializer)
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
				let desc = ImageDescJson::deserialize(MapAccessDeserializer::new(map))?;
				ImageDesc::try_from(desc).map_err(|e| A::Error::custom(e.to_string()))
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

	#[allow(clippy::zero_prefixed_literal)]
	#[test_case(00, ImageDesc::File("/foo/bar.img".into()), "/foo/bar.img")]
	#[test_case(01, ImageDesc::Software{ list: None, name: "abcde".into(), part: None }, "abcde")]
	#[test_case(02, ImageDesc::Software{ list: Some("abc".into()), name: "def".into(), part: None }, "abc:def")]
	#[test_case(03, ImageDesc::Software{ list: Some("abc".into()), name: "def".into(), part: Some("ghi".into()) }, "abc:def:ghi")]
	#[test_case(04, ImageDesc::socket("contoso.com", 8888).unwrap(), "socket.contoso.com:8888")]
	fn as_mame_image_desc(_index: usize, image_desc: ImageDesc, expected: &str) {
		let actual = image_desc.as_mame_image_desc();
		assert_eq!(expected, &*actual);
	}

	#[allow(clippy::zero_prefixed_literal)]
	#[test_case(00, "/foo/bar.img", None, Ok(ImageDesc::File("/foo/bar.img".into())))]
	#[test_case(01, "abcde", None, Ok(ImageDesc::Software { list: None, name: "abcde".into(), part: None }))]
	#[test_case(02, "abc:def", None, Ok(ImageDesc::Software { list: Some("abc".into()), name: "def".into(), part: None }))]
	#[test_case(03, "abc:def:ghi", None, Ok(ImageDesc::Software { list: Some("abc".into()), name: "def".into(), part: Some("ghi".into()) }))]
	#[test_case(04, "socket.contoso.com:8888", None, Ok(ImageDesc::socket("contoso.com", 8888).unwrap()))]
	#[test_case(05, "", None, Err(ThisError::InvalidEmptyString))]
	#[test_case(06, "socket.INVALID", None, Err(ThisError::CannotParseSocketString("socket.INVALID".into())))]
	#[test_case(07, "socket.------:8888", None, Err(ThisError::InvalidHostName("------".into())))]
	#[test_case(08, "socket.contoso.com:", None, Err(ThisError::CannotParsePortNumber))]
	#[test_case(09, "socket.contoso.com:INVALID", None, Err(ThisError::CannotParsePortNumber))]
	#[test_case(10, "socket.contoso.com:888888", None, Err(ThisError::CannotParsePortNumber))]
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
	#[test_case(1, ImageDesc::Software { list: None, name: "abcde".into(), part: None }, r#"{"type":"software","desc":"abcde"}"#)]
	#[test_case(2, ImageDesc::socket("contoso.com", 8888).unwrap(), r#"{"type":"socket","hostname":"contoso.com","port":8888}"#)]
	fn serialize(_index: usize, desc: ImageDesc, expected: &str) {
		let actual = serde_json::to_string(&desc).unwrap();
		assert_eq!(expected, &actual);
	}

	#[test_case(0, r#"{"type":"file","filename":"/alpha/bravo.img"}"#, ImageDesc::File("/alpha/bravo.img".into()))]
	#[test_case(1, r#"{"type":"software","name":"fghijk"}"#, ImageDesc::Software { list: None, name: "fghijk".into(), part: None })]
	#[test_case(1, r#"{"type":"software","desc":"fghijk"}"#, ImageDesc::Software { list: None, name: "fghijk".into(), part: None })]
	#[test_case(1, r#"{"type":"software","desc":"fgh:ijk"}"#, ImageDesc::Software { list: Some("fgh".into()), name: "ijk".into(), part: None })]
	#[test_case(1, r#"{"type":"software","desc":"fgh:ijk:lmn"}"#, ImageDesc::Software { list: Some("fgh".into()), name: "ijk".into(), part: Some("lmn".into()) })]
	#[test_case(2, r#"{"type":"socket","hostname":"litware.com","port":7777}"#, ImageDesc::socket("litware.com", 7777).unwrap())]
	#[test_case(3, "\"/foo/bar.img\"", ImageDesc::File("/foo/bar.img".into()))]
	#[test_case(4, "\"abcde\"", ImageDesc::Software { list: None, name: "abcde".into(), part: None })]
	#[test_case(5, "\"socket.contoso.com:8888\"", ImageDesc::socket("contoso.com", 8888).unwrap())]
	fn deserialize(_index: usize, json: &str, expected: ImageDesc) {
		let actual = serde_json::from_str(json).unwrap();
		assert_eq!(&expected, &actual);
	}
}
