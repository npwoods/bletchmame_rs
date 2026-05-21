use std::borrow::Cow;
use std::io::BufRead;

use anyhow::Result;
use itertools::Itertools;
use slint::StyledText;
use smol_str::SmolStr;
use tracing::info_span;

use crate::history_xml::HistoryXml;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	History,
	Entry,
	EntrySystems,
	EntrySoftware,
	EntryText,
}

const TEXT_CAPTURE_PHASES: &[Phase] = &[Phase::EntryText];

#[derive(Debug, Default)]
struct State {
	phase_stack: Vec<Phase>,
	history: HistoryXml,
	entry_infos: Vec<EntryInfo>,
	text: Option<String>,
}

#[derive(Debug)]
enum EntryInfo {
	System { name: SmolStr },
	Software { list: SmolStr, name: SmolStr },
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Missing mandatory attribute {0} when parsing status XML")]
	MissingMandatoryAttribute(&'static str),
}

impl State {
	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> Result<Option<Phase>> {
		let phase = self.phase_stack.last().unwrap_or(&Phase::Root);
		let new_phase = match (phase, evt.name().as_ref()) {
			(Phase::Root, b"history") => {
				let [_version, _date] = evt.find_attributes([b"version", b"date"])?;
				Some(Phase::History)
			}
			(Phase::History, b"entry") => Some(Phase::Entry),
			(Phase::Entry, b"systems") => Some(Phase::EntrySystems),
			(Phase::Entry, b"software") => Some(Phase::EntrySoftware),
			(Phase::Entry, b"text") => {
				self.text = Some(String::with_capacity(2048));
				Some(Phase::EntryText)
			}
			(Phase::EntrySystems, b"system") => {
				let [name] = evt.find_attributes([b"name"])?;
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?.into();
				let entry_info = EntryInfo::System { name };
				self.entry_infos.push(entry_info);
				Some(Phase::EntrySystems)
			}
			(Phase::EntrySoftware, b"item") => {
				let [list, name] = evt.find_attributes([b"list", b"name"])?;
				let list = list.ok_or(ThisError::MissingMandatoryAttribute("list"))?.into();
				let name = name.ok_or(ThisError::MissingMandatoryAttribute("name"))?.into();
				let entry_info = EntryInfo::Software { list, name };
				self.entry_infos.push(entry_info);
				Some(Phase::EntrySoftware)
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(&mut self, text: Option<String>) -> Result<()> {
		match self.phase_stack.last().unwrap() {
			Phase::Entry => {
				let text = self.text.take().unwrap();
				let markdown = markdown_from_history_text(&text);
				let styled_text = StyledText::from_markdown(&markdown)
					.expect("markdown_from_history_text() should always produce valid markdown");
				for ei in self.entry_infos.drain(..) {
					match ei {
						EntryInfo::System { name } => {
							self.history.systems.insert(name, styled_text.clone());
						}
						EntryInfo::Software { list, name } => {
							self.history.software.insert((list, name), styled_text.clone());
						}
					}
				}
			}
			Phase::EntryText => {
				self.text.as_mut().unwrap().push_str(text.as_deref().unwrap());
			}
			_ => { /* ignore */ }
		}
		Ok(())
	}
}

pub fn parse_from_reader(reader: impl BufRead, callback: impl Fn() -> bool) -> Result<Option<HistoryXml>> {
	let span = info_span!("parse_from_reader");
	let _guard = span.enter();

	let mut reader = XmlReader::from_reader(reader, false);
	let mut buf = Vec::with_capacity(1024);
	let mut state = State {
		phase_stack: Vec::with_capacity(32),
		entry_infos: Vec::with_capacity(32),
		..Default::default()
	};

	while let Some(evt) = reader.next(&mut buf)? {
		if callback() {
			// cancelled!
			return Ok(None);
		}
		match evt {
			XmlEvent::Start(evt) => {
				let new_phase = state.handle_start(evt)?;
				if let Some(new_phase) = new_phase {
					state.phase_stack.push(new_phase);

					if TEXT_CAPTURE_PHASES.contains(&new_phase) {
						reader.start_text_capture();
					}
				} else {
					reader.start_unknown_tag();
				}
			}

			XmlEvent::End(s) => {
				state.handle_end(s)?;
				state.phase_stack.pop().unwrap();
			}

			XmlEvent::Null => {} // meh
		}
	}
	Ok(Some(state.history))
}

fn markdown_from_history_text(text: &str) -> String {
	text.trim()
		.lines()
		.map(|line| {
			let line = if let Some(line) = line.strip_prefix("- ")
				&& let Some(line) = line.strip_suffix(" -")
			{
				Cow::Owned(format!("**{line}**"))
			} else {
				Cow::Borrowed(line)
			};
			markdown_urls(line)
		})
		.join("\n")
}

fn markdown_urls<'a>(text: impl Into<Cow<'a, str>>) -> Cow<'a, str> {
	let text = text.into();
	let mut result = None;
	let mut current = text.as_ref();

	while !current.is_empty() {
		let h = current.find("http://");
		let hs = current.find("https://");

		let start = match (h, hs) {
			(Some(i), Some(j)) => i.min(j),
			(Some(i), None) => i,
			(None, Some(j)) => j,
			(None, None) => break,
		};

		let result = result.get_or_insert_with(|| String::with_capacity(text.len() + 128));
		result.push_str(&current[..start]);
		let remainder = &current[start..];

		let end = remainder.find(|c: char| c.is_whitespace()).unwrap_or(remainder.len());
		let mut url_end = end;

		// backtrack trailing punctuation
		while url_end > 0 {
			let last_char = remainder.as_bytes()[url_end - 1];
			if matches!(
				last_char,
				b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b']' | b'}' | b'\'' | b'"'
			) {
				url_end -= 1;
			} else {
				break;
			}
		}

		if url_end > 0 {
			let url = &remainder[..url_end];
			result.push('[');
			result.push_str(url);
			result.push_str("](");
			result.push_str(url);
			result.push(')');
			current = &remainder[url_end..];
		} else {
			// this should be impossible because we found http:// or https://, but let's be safe
			result.push_str(&remainder[..1]);
			current = &remainder[1..];
		}
	}

	if let Some(mut result) = result {
		result.push_str(current);
		Cow::Owned(result)
	} else {
		text
	}
}

#[cfg(test)]
mod tests {
	use test_case::test_case;

	#[test_case(0, "no url", "no url")]
	#[test_case(1, "http://google.com", "[http://google.com](http://google.com)")]
	#[test_case(2, "https://google.com", "[https://google.com](https://google.com)")]
	#[test_case(3, "Visit http://google.com for info.", "Visit [http://google.com](http://google.com) for info.")]
	#[test_case(4, "Check (https://www.mamedev.org)", "Check ([https://www.mamedev.org](https://www.mamedev.org))")]
	#[test_case(
		5,
		"Multiple: http://one.com and https://two.com!",
		"Multiple: [http://one.com](http://one.com) and [https://two.com](https://two.com)!"
	)]
	fn markdown_urls(_index: usize, input: &str, expected: &str) {
		let actual = super::markdown_urls(input);
		assert_eq!(&actual, expected);
	}
}
