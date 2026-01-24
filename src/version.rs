use std::cmp::Ordering;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

#[derive(Clone, PartialEq)]
pub struct MameVersion {
	major_minor: Option<(u16, u16)>,
	full_text: Option<Box<str>>,
}

impl MameVersion {
	pub const fn new(major: u16, minor: u16) -> Self {
		Self {
			major_minor: Some((major, minor)),
			full_text: None,
		}
	}

	pub fn is_dirty(&self) -> bool {
		self.major_minor.is_none() || self.full_text.is_some()
	}

	pub fn parse_simple(s: impl AsRef<str>) -> Option<Self> {
		let (major, minor) = s.as_ref().split_once('.')?;
		let (major, minor) = (major.parse().ok()?, minor.parse().ok()?);
		Some(Self::new(major, minor))
	}

	fn proxy_key(self: &MameVersion) -> Option<impl Ord + use<>> {
		self.major_minor
			.map(|(major, minor)| (major, minor, self.is_dirty() as u8))
	}
}

impl<T> From<T> for MameVersion
where
	T: AsRef<str> + Into<Box<str>>,
{
	fn from(s: T) -> Self {
		// try to parse out the major and minor versions
		let mut iter = s.as_ref().split([' ', '.']);
		let major = iter.next().and_then(|s| s.parse().ok());
		let minor = iter.next().and_then(|s| s.parse().ok());
		let major_minor = Option::zip(major, minor);

		// try creating a clean version
		let version = major_minor.and_then(|(major, minor)| {
			let version = Self::new(major, minor);
			(s.as_ref() == version.to_string()).then_some(version)
		});

		// we might need to create a dirty version
		version.unwrap_or_else(|| Self {
			major_minor,
			full_text: Some(s.into()),
		})
	}
}

impl PartialOrd for MameVersion {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		let self_key = self.proxy_key()?;
		let other_key = other.proxy_key()?;

		match Ord::cmp(&self_key, &other_key) {
			Ordering::Less => Some(Ordering::Less),
			Ordering::Greater => Some(Ordering::Greater),
			Ordering::Equal => (self.full_text == other.full_text).then_some(Ordering::Equal),
		}
	}
}

impl Display for MameVersion {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		if let Some((major, minor)) = self.major_minor
			&& self.full_text.is_none()
		{
			write!(f, "{major}.{minor} (mame{major}{minor})")
		} else {
			write!(f, "{}", self.full_text.as_deref().unwrap_or_default())
		}
	}
}

impl Debug for MameVersion {
	fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
		Display::fmt(self, f)
	}
}

impl Serialize for MameVersion {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		serializer.serialize_str(&self.to_string())
	}
}

impl<'de> Deserialize<'de> for MameVersion {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let s: &str = Deserialize::deserialize(deserializer)?;
		Ok(Self::from(s))
	}
}

#[cfg(test)]
mod test {
	use std::cmp::Ordering;

	use test_case::test_case;

	use super::MameVersion;

	#[test_case(0, "", true)]
	#[test_case(1, "xyz", true)]
	#[test_case(2, "0.242 (mame0242)", false)]
	#[test_case(3, "0.271 (mame0271)", false)]
	#[test_case(4, "0.271 (mame0271-6790-g78849ebc06c-dirty)", true)]
	fn general(_index: usize, s: &str, expected_is_dirty: bool) {
		let version = MameVersion::from(s);
		let actual = (version.to_string(), version.is_dirty());

		let expected = (s.to_string(), expected_is_dirty);
		assert_eq!(actual, expected);
	}

	#[test_case(0, "xyz", "xyz", None)]
	#[test_case(1, "xyz", "0.242 (mame0242)", None)]
	#[test_case(2, "0.243 (mame0243)", "xyz", None)]
	#[test_case(3, "0.243 (mame0243)", "0.242 (mame0242)", Some(Ordering::Greater))]
	#[test_case(4, "0.243 (mame0243)", "0.243 (mame0243)", Some(Ordering::Equal))]
	#[test_case(5, "0.243 (mame0243)", "0.271 (mame0271)", Some(Ordering::Less))]
	#[test_case(6, "0.243 (mame0243)", "0.271 (mame0271-6790-g78849ebc06c-dirty)", Some(Ordering::Less))]
	#[test_case(7, "0.271 (mame0271)", "0.271 (mame0271-6790-g78849ebc06c-dirty)", Some(Ordering::Less))]
	#[test_case(8, "0.272 (mame0272)", "0.271 (mame0271-6790-g78849ebc06c-dirty)", Some(Ordering::Greater))]
	fn partial_cmp(_index: usize, a: &str, b: &str, expected: Option<Ordering>) {
		let a = MameVersion::from(a);
		let b = MameVersion::from(b);
		let actual: Option<Ordering> = PartialOrd::partial_cmp(&a, &b);
		assert_eq!(expected, actual);
	}
}
