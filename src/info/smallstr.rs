use std::borrow::Borrow;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Debug;
use std::fmt::Error;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;

use arrayvec::ArrayString;
use slint::SharedString;

#[derive(Clone, Copy)]
pub enum SmallStrRef<'a> {
	Ref(&'a str),
	Inline(ArrayString<5>),
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

impl Debug for SmallStrRef<'_> {
	fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
		self.as_ref().fmt(f)
	}
}

impl<'a> AsRef<str> for SmallStrRef<'a> {
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

impl<'a> PartialEq<str> for SmallStrRef<'a> {
	fn eq(&self, other: &str) -> bool {
		self.as_ref() == other
	}
}

impl<'a> PartialEq<SmallStrRef<'a>> for str {
	fn eq(&self, other: &SmallStrRef<'a>) -> bool {
		self == other.as_ref()
	}
}

impl<'a> PartialEq<&str> for SmallStrRef<'a> {
	fn eq(&self, other: &&str) -> bool {
		self.as_ref() == *other
	}
}

impl<'a> PartialEq<SmallStrRef<'a>> for &str {
	fn eq(&self, other: &SmallStrRef<'a>) -> bool {
		*self == other.as_ref()
	}
}

impl<'a> Eq for SmallStrRef<'a> {}

impl<'a> PartialOrd for SmallStrRef<'a> {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(Ord::cmp(self, other))
	}
}

impl<'a> Ord for SmallStrRef<'a> {
	fn cmp(&self, other: &Self) -> Ordering {
		self.as_ref().cmp(other.as_ref())
	}
}

impl<'a> Deref for SmallStrRef<'a> {
	type Target = str;

	fn deref(&self) -> &str {
		self.as_ref()
	}
}

impl<'a> Borrow<str> for SmallStrRef<'a> {
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

	fn calculate_hash<T: Hash>(t: &T) -> u64 {
		let mut s = DefaultHasher::new();
		t.hash(&mut s);
		s.finish()
	}
}
