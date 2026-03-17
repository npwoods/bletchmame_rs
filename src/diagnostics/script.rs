use std::io::BufRead;

use anyhow::Result;

use crate::version::MameVersion;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;

#[derive(Debug)]
pub struct Script {
	#[allow(dead_code)]
	pub required_version: Option<MameVersion>,
	pub commands: Box<[Box<str>]>,
}

impl Script {
	pub fn parse(reader: impl BufRead) -> Result<Self> {
		parse_script(reader)
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	Script,
	Command,
}

const TEXT_CAPTURE_PHASES: &[Phase] = &[Phase::Command];

#[derive(Debug, Default)]
struct State {
	phase_stack: Vec<Phase>,
	required_version: Option<MameVersion>,
	commands: Vec<Box<str>>,
}

impl State {
	fn handle_start(&mut self, evt: XmlElement<'_>) -> Result<Option<Phase>> {
		let phase = self.phase_stack.last().unwrap_or(&Phase::Root);
		let new_phase = match (phase, evt.name().as_ref()) {
			(Phase::Root, b"script") => {
				let [required_version] = evt.find_attributes([b"requiredVersion"])?;
				if let Some(required_version) = required_version {
					let required_version = MameVersion::from(required_version.as_ref());
					self.required_version = Some(required_version);
				}
				Some(Phase::Script)
			}
			(Phase::Script, b"command") => Some(Phase::Command),
			_ => None,
		};
		Ok(new_phase)
	}

	fn handle_end(&mut self, text: Option<String>) -> Result<()> {
		match self.phase_stack.last().unwrap_or(&Phase::Root) {
			Phase::Command => {
				let command_text = text.unwrap().into();
				self.commands.push(command_text);
			}
			_ => { /* ignore */ }
		}
		Ok(())
	}
}

fn parse_script(reader: impl BufRead) -> Result<Script> {
	let mut xml_reader = XmlReader::from_reader(reader, true);
	let mut buf = Vec::new();
	let mut state = State::default();
	state.phase_stack.push(Phase::Root);

	while let Some(evt) = xml_reader.next(&mut buf)? {
		match evt {
			XmlEvent::Start(elem) => {
				let new_phase = state.handle_start(elem)?;
				if let Some(phase) = new_phase {
					state.phase_stack.push(phase);

					if TEXT_CAPTURE_PHASES.contains(&phase) {
						xml_reader.start_text_capture();
					}
				} else {
					xml_reader.start_unknown_tag();
				}
			}
			XmlEvent::End(s) => {
				state.handle_end(s)?;
				state.phase_stack.pop().unwrap();
			}
			_ => {}
		}
	}

	let required_version = state.required_version;
	let commands = state.commands.into();
	let script = Script {
		required_version,
		commands,
	};
	Ok(script)
}
