#![allow(dead_code)]
use std::collections::HashSet;
use std::io::BufRead;
use std::rc::Rc;

use crate::error::BoxDynError;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;
use crate::Error;
use crate::Result;

struct SoftwareList {
	name: Rc<str>,
	description: Rc<str>,
	software: Vec<Software>,
}

struct Software {
	name: Rc<str>,
	description: Rc<str>,
	year: Rc<str>,
	publisher: Rc<str>,
}

impl SoftwareList {
	pub fn from_reader(reader: impl BufRead) -> Result<Self> {
		let mut state = State::new();
		let mut reader = XmlReader::from_reader(reader);
		let mut buf = Vec::with_capacity(1024);

		while let Some(evt) = reader.next(&mut buf).map_err(|e| softlistxml_err(&reader, e))? {
			match evt {
				XmlEvent::Start(evt) => {
					let new_phase = state.handle_start(evt).map_err(|e| softlistxml_err(&reader, e))?;

					if let Some(new_phase) = new_phase {
						state.phase = new_phase;

						if TEXT_CAPTURE_PHASES.contains(&state.phase) {
							reader.start_text_capture();
						}
					} else {
						reader.start_unknown_tag();
					}
				}

				XmlEvent::End(s) => {
					let new_phase = state.handle_end(s).map_err(|e| softlistxml_err(&reader, e))?;
					state.phase = new_phase;
				}

				XmlEvent::Null => {} // meh
			}
		}

		assert_eq!(Phase::Root, state.phase);
		Ok(state.software_list)
	}
}

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

struct State {
	phase: Phase,
	strings: HashSet<Rc<str>>,
	empty_str: Rc<str>,
	software_list: SoftwareList,
}

impl State {
	pub fn new() -> Self {
		let mut strings = HashSet::new();
		let empty_str = Rc::<str>::from("");
		strings.insert(empty_str.clone());

		Self {
			phase: Phase::Root,
			strings: HashSet::new(),
			empty_str: empty_str.clone(),
			software_list: SoftwareList {
				name: empty_str.clone(),
				description: empty_str.clone(),
				software: Vec::new(),
			},
		}
	}

	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> std::result::Result<Option<Phase>, BoxDynError> {
		let new_phase = match (self.phase, evt.name().as_ref()) {
			(Phase::Root, b"softwarelist") => {
				let [name, description] = evt.find_attributes([b"name", b"description"])?;
				self.software_list.name = self.string(&name.unwrap_or_default());
				self.software_list.description = self.string(&description.unwrap_or_default());
				Some(Phase::SoftwareList)
			}
			(Phase::SoftwareList, b"software") => {
				let [name] = evt.find_attributes([b"name"])?;
				let name = self.string(&name.unwrap_or_default());

				let software = Software {
					name,
					description: self.empty_str.clone(),
					year: self.empty_str.clone(),
					publisher: self.empty_str.clone(),
				};
				self.software_list.software.push(software);
				Some(Phase::Software)
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(&mut self, text: Option<String>) -> std::result::Result<Phase, BoxDynError> {
		let new_phase = match self.phase {
			Phase::Root => panic!(),
			Phase::SoftwareList => Phase::Root,
			Phase::Software => Phase::SoftwareList,

			Phase::SoftwareDescription => {
				let description = self.string(&text.unwrap());
				self.software_list.software.last_mut().unwrap().description = description;
				Phase::Software
			}
			Phase::SoftwareYear => {
				let year = self.string(&text.unwrap());
				self.software_list.software.last_mut().unwrap().year = year;
				Phase::Software
			}
			Phase::SoftwarePublisher => {
				let publisher = self.string(&text.unwrap());
				self.software_list.software.last_mut().unwrap().publisher = publisher;
				Phase::Software
			}
		};
		Ok(new_phase)
	}

	fn string(&mut self, s: &str) -> Rc<str> {
		self.strings.get(s).cloned().unwrap_or_else(|| {
			let result = Rc::<str>::from(s);
			self.strings.insert(result.clone());
			result
		})
	}
}

fn softlistxml_err(reader: &XmlReader<impl BufRead>, e: BoxDynError) -> crate::error::Error {
	Error::SoftwareListXmlParsing(reader.buffer_position(), e)
}

#[cfg(test)]
mod test {
	use std::io::BufReader;

	use test_case::test_case;

	use super::SoftwareList;

	#[test_case(0, include_str!("test_data/softlist_coco_cart.xml"), ("coco_cart", "Tandy Radio Shack Color Computer cartridges", 112))]
	#[test_case(1, include_str!("test_data/softlist_msx1_cart.xml"), ("msx1_cart", "MSX1 cartridges", 1230))]
	pub fn general(_index: usize, xml: &str, expected: (&str, &str, usize)) {
		let reader = BufReader::new(xml.as_bytes());
		let software_list = SoftwareList::from_reader(reader);
		let actual = software_list
			.as_ref()
			.map(|x| (x.name.as_ref(), x.description.as_ref(), x.software.len()))
			.map_err(|x| format!("{x}"));
		assert_eq!(Ok(expected), actual);
	}
}
