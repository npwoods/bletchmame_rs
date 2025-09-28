use std::io::BufRead;

use anyhow::Result;
use slint::ToSharedString;
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
				let text = self.text.take().unwrap().trim().to_shared_string();
				for ei in self.entry_infos.drain(..) {
					match ei {
						EntryInfo::System { name } => {
							self.history.systems.insert(name, text.clone());
						}
						EntryInfo::Software { list, name } => {
							self.history.software.insert((list, name), text.clone());
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
