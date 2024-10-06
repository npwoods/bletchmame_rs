/// quick-xml is a good API but this provides some common handling
use std::borrow::Cow;
use std::fmt::Debug;
use std::io::BufRead;
use std::str::from_utf8;

use quick_xml::escape::unescape;
use quick_xml::events::BytesStart;
use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::Reader;

use crate::error::BoxDynError;

/// quick-xml events are at a slightly different granularity than what we would prefer
#[derive(Debug)]
pub enum XmlEvent<'a> {
	Null,
	Start(XmlElement<'a>),
	End(Option<String>),
}

pub struct XmlReader<R> {
	reader: CurrentReader<R>,
	next_event_is_end: bool,
	known_depth: u32,
	unknown_depth: u32,
	current_text: Option<String>,
	read_to_end: bool,
}

enum CurrentReader<R> {
	Active(Reader<R>),
	Done(u64),
}

impl<R> XmlReader<R>
where
	R: BufRead,
{
	pub fn from_reader(reader: R, read_to_end: bool) -> Self {
		Self {
			reader: CurrentReader::Active(Reader::from_reader(reader)),
			next_event_is_end: false,
			known_depth: 0,
			unknown_depth: 0,
			current_text: None,
			read_to_end,
		}
	}

	pub fn next<'a>(&mut self, buf: &'a mut Vec<u8>) -> std::result::Result<Option<XmlEvent<'a>>, BoxDynError> {
		let result = self.internal_next(buf);

		// if we've reached the end of file, clear out the reader
		if let CurrentReader::Active(reader) = &self.reader {
			if !matches!(result, Ok(Some(_))) {
				let buffer_position = reader.buffer_position();
				self.reader = CurrentReader::Done(buffer_position);
			}
		}

		result
	}

	fn internal_next<'a>(&mut self, buf: &'a mut Vec<u8>) -> std::result::Result<Option<XmlEvent<'a>>, BoxDynError> {
		let event = if self.next_event_is_end {
			self.next_event_is_end = false;
			Some(XmlEvent::End(self.current_text.take()))
		} else if let CurrentReader::Active(reader) = &mut self.reader {
			match reader.read_event_into(buf)? {
				Event::Eof => None,
				Event::Start(bytes_start) => Some(XmlEvent::Start(XmlElement { bytes_start })),
				Event::End(_) => Some(XmlEvent::End(self.current_text.take())),
				Event::Empty(bytes_start) => {
					self.next_event_is_end = true;
					Some(XmlEvent::Start(XmlElement { bytes_start }))
				}
				Event::Text(bytes_text) => {
					if let Some(current_text) = &mut self.current_text {
						let string = cow_bytes_to_str(bytes_text.into_inner())?;
						current_text.push_str(&string);
					}
					Some(XmlEvent::Null)
				}
				_ => Some(XmlEvent::Null),
			}
		} else {
			None
		};

		// what sort of adjustment do we need to make?
		let depth_adjustment = match event {
			Some(XmlEvent::Start(_)) => 1,
			Some(XmlEvent::End(_)) => -1,
			_ => 0,
		};

		// we know what type of event this is, but are we in "unknown tags?"
		let (event, depth) = if self.unknown_depth == 0 || event.is_none() {
			(event, &mut self.known_depth)
		} else {
			(Some(XmlEvent::Null), &mut self.unknown_depth)
		};

		// adjust the known or unknown depth
		*depth = depth
			.checked_add_signed(depth_adjustment)
			.ok_or_else(|| BoxDynError::from("Invalid close tag"))?;

		// did we hit the last close tag, and we're not reading until the end?
		let event = (self.read_to_end || self.known_depth > 0 || self.unknown_depth > 0 || depth_adjustment != -1)
			.then_some(event)
			.flatten();

		// are we at the end of file, but still in a tag?
		if event.is_none() && (self.known_depth > 0 || self.unknown_depth > 0) {
			self.known_depth = 0;
			self.unknown_depth = 0;
			return Err(BoxDynError::from("Unexpected end of file"));
		}

		// and return!
		Ok(event)
	}

	pub fn start_unknown_tag(&mut self) {
		assert_eq!(self.unknown_depth, 0);
		self.known_depth -= 1;
		self.unknown_depth += 1;
	}

	pub fn start_text_capture(&mut self) {
		self.current_text = Some(String::new());
	}

	pub fn buffer_position(&self) -> u64 {
		match &self.reader {
			CurrentReader::Active(reader) => reader.buffer_position(),
			CurrentReader::Done(buffer_position) => *buffer_position,
		}
	}
}

pub struct XmlElement<'a> {
	bytes_start: BytesStart<'a>,
}

impl<'a> XmlElement<'a> {
	pub fn name(&'a self) -> QName<'a> {
		self.bytes_start.name()
	}

	pub fn find_attributes<const N: usize>(
		&'a self,
		attrs: [&[u8]; N],
	) -> std::result::Result<[Option<Cow<'a, str>>; N], BoxDynError> {
		const DEFAULT_ATTRVAL: Option<Cow<str>> = None;
		let mut result: [Option<Cow<'a, str>>; N] = [DEFAULT_ATTRVAL; N];

		for attribute in self.bytes_start.attributes() {
			let attribute = attribute?;
			let attr_name = attribute.key.as_ref();
			let pos = attrs
				.iter()
				.enumerate()
				.filter_map(|(index, &target)| (target == attr_name).then_some(index))
				.next();
			if let Some(pos) = pos {
				assert_eq!(None, result[pos]);
				if let Ok(attr_value) = cow_bytes_to_str(attribute.value) {
					result[pos] = Some(attr_value);
				}
			}
		}

		Ok(result)
	}
}

impl<'a> Debug for XmlElement<'a> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "<{}", String::from_utf8_lossy(self.name().as_ref()))?;
		for x in self.bytes_start.attributes().with_checks(false) {
			let attribute = x.unwrap();
			write!(
				f,
				" {}=\"{}\"",
				String::from_utf8_lossy(attribute.key.as_ref()),
				String::from_utf8_lossy(attribute.value.as_ref())
			)?;
		}
		write!(f, ">")?;
		Ok(())
	}
}

fn cow_bytes_to_str(cow: Cow<'_, [u8]>) -> std::result::Result<Cow<'_, str>, BoxDynError> {
	match cow {
		Cow::Borrowed(bytes) => {
			let s = from_utf8(bytes)?;
			Ok(unescape(s)?)
		}
		Cow::Owned(bytes) => {
			let s = from_utf8(&bytes)?;
			let s = unescape(s)?;
			Ok(s.into_owned().into())
		}
	}
}

#[cfg(test)]
mod test {
	use std::borrow::Cow;
	use std::str::from_utf8;

	use assert_matches::assert_matches;
	use test_case::test_case;

	use super::XmlEvent;
	use super::XmlReader;

	#[derive(Debug, PartialEq, Eq)]
	pub enum Part {
		Start(&'static str),
		End(Option<&'static str>),
		Error,
	}

	#[test_case(0, "<foo><bar/></foo>", true, &[Part::Start("foo"), Part::Start("bar"), Part::End(None), Part::End(None)])]
	#[test_case(1, "<blah><unknown/></blah>", true, &[Part::Start("blah"), Part::End(None)])]
	#[test_case(2, "<alpha><bravo/><unknown/><charlie/></alpha>", true, &[Part::Start("alpha"), Part::Start("bravo"), Part::End(None), Part::Start("charlie"), Part::End(None), Part::End(None)])]
	#[test_case(3, "<alpha><text>Hello</text></alpha>", true, &[Part::Start("alpha"), Part::Start("text"), Part::End(Some("Hello")), Part::End(None)])]
	#[test_case(4, "<foo><bar/>", true, &[Part::Start("foo"), Part::Start("bar"), Part::End(None), Part::Error])]
	#[test_case(5, "</foo>", true, &[Part::Error])]
	#[test_case(6, "<foo/></bar>", true, &[Part::Start("foo"), Part::End(None), Part::Error])]
	#[test_case(7, "<foo/>BLAH", true, &[Part::Start("foo"), Part::End(None), Part::Error])]
	#[test_case(8, "<foo/>BLAH", false, &[Part::Start("foo"), Part::End(None)])]
	pub fn general(_index: usize, xml: &str, read_to_end: bool, expected: &[Part]) {
		let mut reader = XmlReader::from_reader(xml.as_bytes(), read_to_end);
		let mut buf = Vec::new();

		let mut actual = Vec::new();
		while let Some(event) = reader.next(&mut buf).transpose() {
			match event {
				Ok(XmlEvent::Start(x)) => {
					let name = x.name();
					let name = from_utf8(name.as_ref()).unwrap();
					if name == "unknown" {
						reader.start_unknown_tag();
					} else {
						if name == "text" {
							reader.start_text_capture();
						}
						let s = String::leak(name.to_string());
						actual.push(Part::Start(s));
					}
				}
				Ok(XmlEvent::End(s)) => {
					let s = s.map(|s| String::leak(s) as &str);
					actual.push(Part::End(s));
				}
				Err(_) => {
					actual.push(Part::Error);
				}
				_ => {}
			}
		}
		assert_eq!(expected, &actual);
	}

	#[test_case(0, "<foo><bar/></foo>", "")]
	#[test_case(1, "<foo><bar/></foo>BLAH", "BLAH")]
	pub fn extra_data(_index: usize, xml: &str, expected: &str) {
		let mut xml_bytes = xml.as_bytes();
		let mut xml_reader = XmlReader::from_reader(&mut xml_bytes, false);
		let mut buf = Vec::new();
		while let Some(event) = xml_reader.next(&mut buf).transpose() {
			assert_matches!(event, Ok(_));
		}

		let actual = from_utf8(xml_bytes);
		assert_eq!(Ok(expected), actual);
	}

	#[test_case(0, Cow::Borrowed(b""), Ok(""))]
	#[test_case(1, Cow::Owned(b"".into()), Ok(""))]
	#[test_case(2, Cow::Borrowed(b"foo"), Ok("foo"))]
	#[test_case(3, Cow::Owned(b"foo".into()), Ok("foo"))]
	#[test_case(4, Cow::Borrowed(b"&lt;escaping&gt; &amp; things"), Ok("<escaping> & things"))]
	#[test_case(5, Cow::Owned(b"&lt;escaping&gt; &amp; things".into()), Ok("<escaping> & things"))]
	pub fn cow_bytes_to_str(_index: usize, input: Cow<'_, [u8]>, expected: std::result::Result<&str, ()>) {
		let actual = super::cow_bytes_to_str(input);
		let actual = actual.as_ref().map_or_else(|_| Err(()), |x| Ok(x.as_ref()));
		assert_eq!(expected, actual);
	}

	#[test_case(0, "<root alpha=\"aaa\" bravo=\"bbb\" charlie=\"ccc\"/>", [b"alpha", b"bravo", b"charlie"], &[Some("aaa"), Some("bbb"), Some("ccc")])]
	#[test_case(1, "<root alpha=\"ddd\" bravo=\"eee\" charlie=\"fff\"/>", [b"alpha", b"delta", b"echo"], &[Some("ddd"), None, None])]
	#[test_case(2, "<root alpha=\"ggg\"/>", [b"alpha", b"bravo", b"charlie"], &[Some("ggg"), None, None])]
	pub fn find_attributes(_index: usize, xml: &str, attributes: [&[u8]; 3], expected: &[Option<&str>]) {
		let mut reader = XmlReader::from_reader(xml.as_bytes(), true);
		let mut buf = Vec::new();

		while let Some(event) = reader.next(&mut buf).unwrap() {
			if let XmlEvent::Start(elem) = &event {
				let actual = elem.find_attributes(attributes).unwrap();
				let actual = actual
					.iter()
					.map(|x| x.as_ref().map(|y| y.as_ref()))
					.collect::<Vec<_>>();
				assert_eq!(expected, &actual);
			}
		}
	}
}
