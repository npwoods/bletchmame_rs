use std::io::BufRead;
use std::sync::Arc;

use anyhow::Error;
use anyhow::Result;
use tracing::error;

use crate::software::Software;
use crate::software::SoftwareList;
use crate::software::SoftwarePart;
use crate::software::is_valid_software_list_name;
use crate::software::strings::StringDispenser;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	SoftwareList,
	Software,
	SoftwareDescription,
	SoftwareYear,
	SoftwarePublisher,
}

const TEXT_CAPTURE_PHASES: &[Phase] = &[
	Phase::SoftwareDescription,
	Phase::SoftwareYear,
	Phase::SoftwarePublisher,
];

struct State<'a> {
	phase_stack: Vec<Phase>,
	string_dispenser: &'a StringDispenser,
	empty_str: Arc<str>,
	software_list: SoftwareList,
	current_software: Option<Software>,
}

impl<'a> State<'a> {
	pub fn new(string_dispenser: &'a StringDispenser) -> Self {
		let empty_str = string_dispenser.get("");

		Self {
			phase_stack: Vec::with_capacity(32),
			string_dispenser,
			empty_str: empty_str.clone(),
			software_list: SoftwareList {
				name: empty_str.clone(),
				description: empty_str.clone(),
				software: Vec::new(),
			},
			current_software: None,
		}
	}

	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> Result<Option<Phase>> {
		let phase = self.phase_stack.last().unwrap_or(&Phase::Root);
		let new_phase = match (phase, evt.name().as_ref()) {
			(Phase::Root, b"softwarelist") => {
				let [name, description] = evt.find_attributes([b"name", b"description"])?;
				self.software_list.name = self.string_dispenser.get(&name.unwrap_or_default());
				self.software_list.description = self.string_dispenser.get(&description.unwrap_or_default());
				Some(Phase::SoftwareList)
			}
			(Phase::SoftwareList, b"software") => {
				let [name] = evt.find_attributes([b"name"])?;
				let Some(name) = name else {
					error!("handle_start(): Missing name attribute");
					return Ok(None);
				};
				if !is_valid_software_list_name(name.as_ref()) {
					error!("handle_start(): Invalid software name {}", name.as_ref());
					return Ok(None);
				}

				let name = self.string_dispenser.get(&name);
				let software = Software {
					name,
					description: self.empty_str.clone(),
					year: self.empty_str.clone(),
					publisher: self.empty_str.clone(),
					parts: Vec::new(),
				};
				self.current_software = Some(software);
				Some(Phase::Software)
			}
			(Phase::Software, b"description") => Some(Phase::SoftwareDescription),
			(Phase::Software, b"year") => Some(Phase::SoftwareYear),
			(Phase::Software, b"publisher") => Some(Phase::SoftwarePublisher),
			(Phase::Software, b"part") => {
				let [name, interface] = evt.find_attributes([b"name", b"interface"])?;
				if let Some((name, interface)) = Option::zip(name, interface) {
					let (name, interface) = (name.into(), interface.into());
					let part = SoftwarePart { name, interface };
					self.current_software.as_mut().unwrap().parts.push(part);
				}
				None
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(&mut self, text: Option<String>) -> Result<()> {
		match self.phase_stack.last().unwrap_or(&Phase::Root) {
			Phase::Software => {
				let software = self.current_software.take().unwrap().into();
				self.software_list.software.push(software);
			}

			Phase::SoftwareDescription => {
				let description = self.string_dispenser.get(&text.unwrap());
				self.current_software.as_mut().unwrap().description = description;
			}
			Phase::SoftwareYear => {
				let year = self.string_dispenser.get(&text.unwrap());
				self.current_software.as_mut().unwrap().year = year;
			}
			Phase::SoftwarePublisher => {
				let publisher = self.string_dispenser.get(&text.unwrap());
				self.current_software.as_mut().unwrap().publisher = publisher;
			}
			_ => {}
		};
		Ok(())
	}
}

fn softlistxml_err(reader: &XmlReader<impl BufRead>, e: impl Into<Error>) -> Error {
	let message = format!(
		"Error parsing software list XML at position {}",
		reader.buffer_position()
	);
	e.into().context(message)
}

pub fn process_xml(reader: impl BufRead, string_dispenser: &StringDispenser) -> Result<SoftwareList> {
	let mut state = State::new(string_dispenser);
	let mut reader = XmlReader::from_reader(reader, true);
	let mut buf = Vec::with_capacity(1024);

	while let Some(evt) = reader.next(&mut buf).map_err(|e| softlistxml_err(&reader, e))? {
		match evt {
			XmlEvent::Start(evt) => {
				let new_phase = state.handle_start(evt).map_err(|e| softlistxml_err(&reader, e))?;

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
				state.handle_end(s).map_err(|e| softlistxml_err(&reader, e))?;
				state.phase_stack.pop().unwrap();
			}

			XmlEvent::Null => {} // meh
		}
	}

	assert!(state.phase_stack.is_empty());
	Ok(state.software_list)
}

#[cfg(test)]
mod test {
	use std::io::BufReader;

	use test_case::test_case;

	use crate::software::strings::StringDispenser;

	use super::process_xml;

	#[test_case(0, include_str!("test_data/softlist_coco_cart.xml"), ("coco_cart", "Tandy Radio Shack Color Computer cartridges", 112))]
	#[test_case(1, include_str!("test_data/softlist_msx1_cart.xml"), ("msx1_cart", "MSX1 cartridges", 1230))]
	pub fn general(_index: usize, xml: &str, expected: (&str, &str, usize)) {
		let reader = BufReader::new(xml.as_bytes());
		let string_dispenser = StringDispenser::default();
		let software_list = process_xml(reader, &string_dispenser);
		let actual = software_list
			.as_ref()
			.map(|x| (x.name.as_ref(), x.description.as_ref(), x.software.len()))
			.map_err(|x| format!("{x}"));
		assert_eq!(Ok(expected), actual);
	}

	#[test_case(0, include_str!("test_data/softlist_coco_cart.xml"), "clowns", ("Clowns & Balloons", "1982", "Tandy"))]
	pub fn software(_index: usize, xml: &str, name: &str, expected: (&str, &str, &str)) {
		let reader = BufReader::new(xml.as_bytes());
		let string_dispenser = StringDispenser::default();
		let software_list = process_xml(reader, &string_dispenser).unwrap();
		let software = software_list
			.software
			.iter()
			.find(|x| x.name.as_ref() == name)
			.unwrap()
			.as_ref();
		let actual = (
			software.description.as_ref(),
			software.year.as_ref(),
			software.publisher.as_ref(),
		);
		assert_eq!(expected, actual);
	}
}
