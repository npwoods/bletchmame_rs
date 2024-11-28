use std::borrow::Cow;
use std::fmt::Debug;
use std::fmt::Formatter;

pub struct DebugString(Cow<'static, str>);

impl DebugString {
	pub fn elipsis<T>(_value: T) -> Self {
		Self::from("...")
	}
}

impl Debug for DebugString {
	fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
		write!(fmt, "{}", self.0)
	}
}

impl From<&'static str> for DebugString {
	fn from(value: &'static str) -> Self {
		Self(value.into())
	}
}
