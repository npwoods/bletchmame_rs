use std::borrow::Borrow;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Error;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;

use arrayvec::ArrayString;
use itertools::Either;
use slint::SharedString;

#[derive(Clone, Copy)]
pub enum SmallStrRef<'a> {
	Ref(&'a str),
	Inline(ArrayString<5>),
}

impl<'a> SmallStrRef<'a> {
	pub fn split(&self, pattern: char) -> impl Iterator<Item = Cow<'a, str>> {
		match self {
			Self::Ref(s) => Either::Left(s.split(pattern).map(Cow::Borrowed)),
			Self::Inline(s) => Either::Right(
				s.as_str()
					.split(pattern)
					.map(|x| x.to_string())
					.map(Cow::Owned)
					.collect::<Vec<_>>()
					.into_iter(),
			),
		}
	}
}

impl SmallStrRef<'static> {
	pub fn from_small_chars(iter: impl Iterator<Item = char>) -> Self {
		let mut s = ArrayString::default();
		for ch in iter {
			s.push(ch);
		}
		Self::Inline(s)
	}
}

impl<'a> From<&'a str> for SmallStrRef<'a> {
	fn from(value: &'a str) -> Self {
		Self::Ref(value)
	}
}

impl From<SmallStrRef<'_>> for String {
	fn from(value: SmallStrRef<'_>) -> Self {
		value.as_ref().to_string()
	}
}

impl<'a> From<SmallStrRef<'a>> for Cow<'a, str> {
	fn from(value: SmallStrRef<'a>) -> Self {
		match value {
			SmallStrRef::Ref(x) => Cow::Borrowed(x),
			SmallStrRef::Inline(x) => Cow::Owned(x.to_string()),
		}
	}
}

impl From<SmallStrRef<'_>> for SharedString {
	fn from(value: SmallStrRef<'_>) -> Self {
		value.as_ref().into()
	}
}

impl Default for SmallStrRef<'_> {
	fn default() -> Self {
		Self::Inline(Default::default())
	}
}

impl Display for SmallStrRef<'_> {
	fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
		Display::fmt(self.as_ref(), f)
	}
}

impl Debug for SmallStrRef<'_> {
	fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
		Display::fmt(self.as_ref(), f)
	}
}

impl AsRef<str> for SmallStrRef<'_> {
	fn as_ref(&self) -> &str {
		match self {
			SmallStrRef::Ref(x) => x,
			SmallStrRef::Inline(x) => x,
		}
	}
}

impl PartialEq for SmallStrRef<'_> {
	fn eq(&self, other: &Self) -> bool {
		self.as_ref() == other.as_ref()
	}
}

impl PartialEq<str> for SmallStrRef<'_> {
	fn eq(&self, other: &str) -> bool {
		self.as_ref() == other
	}
}

impl<'a> PartialEq<SmallStrRef<'a>> for str {
	fn eq(&self, other: &SmallStrRef<'a>) -> bool {
		self == other.as_ref()
	}
}

impl PartialEq<&str> for SmallStrRef<'_> {
	fn eq(&self, other: &&str) -> bool {
		self.as_ref() == *other
	}
}

impl<'a> PartialEq<SmallStrRef<'a>> for &str {
	fn eq(&self, other: &SmallStrRef<'a>) -> bool {
		*self == other.as_ref()
	}
}

impl Eq for SmallStrRef<'_> {}

impl PartialOrd for SmallStrRef<'_> {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(Ord::cmp(self, other))
	}
}

impl Ord for SmallStrRef<'_> {
	fn cmp(&self, other: &Self) -> Ordering {
		self.as_ref().cmp(other.as_ref())
	}
}

impl Deref for SmallStrRef<'_> {
	type Target = str;

	fn deref(&self) -> &str {
		self.as_ref()
	}
}

impl Borrow<str> for SmallStrRef<'_> {
	fn borrow(&self) -> &str {
		self.as_ref()
	}
}

impl Hash for SmallStrRef<'_> {
	fn hash<H>(&self, state: &mut H)
	where
		H: Hasher,
	{
		self.as_ref().hash(state)
	}
}

#[cfg(test)]
mod test {
	use std::hash::DefaultHasher;
	use std::hash::Hash;
	use std::hash::Hasher;

	use test_case::test_case;

	use super::SmallStrRef;

	#[test_case(0, "")]
	#[test_case(1, "Tiny")]
	#[test_case(2, "ReallyReallyLarge")]
	pub fn eq(_index: usize, s: &str) {
		let small = SmallStrRef::from(s);
		assert_eq!(s, small);
		assert_eq!(calculate_hash(&s), calculate_hash(&small));
	}

	#[test_case(0, "")]
	#[test_case(1, "ABCD")]
	pub fn eq_heterogeneous(_index: usize, s: &str) {
		let s1 = SmallStrRef::from(s);
		let s2 = SmallStrRef::from_small_chars(s.chars());
		assert_eq!(s1, s2);
		assert_eq!(calculate_hash(&s1), calculate_hash(&s2));
	}

	#[test_case(0, "", ',', &[""])]
	#[test_case(1, "foo", ',', &["foo"])]
	#[test_case(2, "foo,bar", ',', &["foo", "bar"])]
	#[test_case(3, "foo\0bar", '\0', &["foo", "bar"])]
	pub fn split(_index: usize, s: &str, pattern: char, expected: &[&str]) {
		let s = SmallStrRef::from(s);
		let actual = s.split(pattern).collect::<Vec<_>>();
		let actual = actual.iter().map(|x| x.as_ref()).collect::<Vec<_>>();
		assert_eq!(expected, actual);
	}

	fn calculate_hash<T: Hash>(t: &T) -> u64 {
		let mut s = DefaultHasher::new();
		t.hash(&mut s);
		s.finish()
	}
}
