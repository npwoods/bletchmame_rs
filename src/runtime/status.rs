use std::io::BufRead;

use serde::Deserialize;
use serde::Serialize;

use crate::error::BoxDynError;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;
use crate::Error;
use crate::Result;

#[derive(Debug, Default)]
pub struct Status {
	pub has_initialized: bool,
	pub machine_name: Option<String>,
}

impl Status {
	pub fn merge(&mut self, update: StatusUpdate) {
		self.machine_name = update.machine_name;
		self.has_initialized = true;
	}
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusUpdate {
	pub machine_name: Option<String>,
}

impl StatusUpdate {
	pub fn parse(reader: impl BufRead) -> Result<Self> {
		parse_status(reader)
	}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
	#[default]
	Root,
	Status,
}

#[derive(Debug, Default)]
struct State {
	phase: Phase,
	machine_name: Option<String>,
}

impl State {
	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> std::result::Result<Option<Phase>, BoxDynError> {
		let new_phase = match (self.phase, evt.name().as_ref()) {
			(Phase::Root, b"status") => {
				let [romname] = evt.find_attributes([b"romname"])?;
				self.machine_name = romname.and_then(|x| (!x.is_empty()).then(|| x.to_string()));
				Some(Phase::Status)
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(&mut self, _text: Option<String>) -> std::result::Result<Phase, BoxDynError> {
		let new_phase = match self.phase {
			Phase::Root => panic!(),
			Phase::Status => Phase::Root,
		};
		Ok(new_phase)
	}
}

fn parse_status(reader: impl BufRead) -> Result<StatusUpdate> {
	let mut reader = XmlReader::from_reader(reader, false);
	let mut buf = Vec::with_capacity(1024);
	let mut state = State::default();

	while let Some(evt) = reader.next(&mut buf).map_err(|e| statusxml_err(&reader, e))? {
		match evt {
			XmlEvent::Start(evt) => {
				let new_phase = state.handle_start(evt).map_err(|e| statusxml_err(&reader, e))?;
				if let Some(new_phase) = new_phase {
					state.phase = new_phase;
				} else {
					reader.start_unknown_tag();
				}
			}

			XmlEvent::End(s) => {
				let new_phase = state.handle_end(s).map_err(|e| statusxml_err(&reader, e))?;
				state.phase = new_phase;
			}

			XmlEvent::Null => {} // meh
		}
	}

	let result = StatusUpdate {
		machine_name: state.machine_name,
	};
	Ok(result)
}

fn statusxml_err(reader: &XmlReader<impl BufRead>, e: BoxDynError) -> crate::error::Error {
	Error::StatusXmlProcessing(reader.buffer_position(), e)
}
