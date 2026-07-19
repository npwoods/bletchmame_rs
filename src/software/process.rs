use std::io::BufRead;

use anyhow::Error;
use anyhow::Result;
use parse_int::parse;
use smol_str::SmolStr;
use tracing::error;

use crate::assethash::AssetHash;
use crate::software::AssetStatus;
use crate::software::Software;
use crate::software::SoftwareAsset;
use crate::software::SoftwareDataArea;
use crate::software::SoftwareList;
use crate::software::SoftwarePart;
use crate::software::is_valid_software_list_name;
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
	SoftwarePart,
	SoftwarePartDataArea,
}

const TEXT_CAPTURE_PHASES: &[Phase] = &[
	Phase::SoftwareDescription,
	Phase::SoftwareYear,
	Phase::SoftwarePublisher,
];

struct State {
	phase_stack: Vec<Phase>,
	software_list: SoftwareList,
	current_software: Option<CurrentSoftware>,
	current_data_areas: Option<Vec<SoftwareDataArea>>,
	current_assets: Option<Vec<SoftwareAsset>>,
}

#[derive(Debug, Default)]
struct CurrentSoftware {
	pub name: SmolStr,
	pub description: SmolStr,
	pub year: SmolStr,
	pub publisher: SmolStr,
	pub parts: Vec<SoftwarePart>,
}

impl State {
	pub fn new() -> Self {
		Self {
			phase_stack: Vec::with_capacity(32),
			software_list: SoftwareList {
				name: Default::default(),
				description: Default::default(),
				software: Vec::new(),
			},
			current_software: None,
			current_data_areas: None,
			current_assets: None,
		}
	}

	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> Result<Option<Phase>> {
		let phase = self.phase_stack.last().unwrap_or(&Phase::Root);
		let new_phase = match (phase, evt.name().as_ref()) {
			(Phase::Root, b"softwarelist") => {
				let [name, description] = evt.find_attributes([b"name", b"description"])?;
				self.software_list.name = name.unwrap_or_default().into();
				self.software_list.description = description.unwrap_or_default().into();
				Some(Phase::SoftwareList)
			}
			(Phase::SoftwareList, b"software") => {
				let [name] = evt.find_attributes([b"name"])?;
				let Some(name) = name else {
					error!("handle_start(): Missing name attribute");
					return Ok(None);
				};
				if !is_valid_software_list_name(name.as_ref()) {
					error!("handle_start(): Invalid software name {}", name);
					return Ok(None);
				}

				let software = CurrentSoftware {
					name: name.into(),
					..Default::default()
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
					let part = SoftwarePart {
						name,
						interface,
						data_areas: Default::default(),
					};
					self.current_software.as_mut().unwrap().parts.push(part);
				}
				self.current_data_areas = Some(Vec::new());
				Some(Phase::SoftwarePart)
			}
			(Phase::SoftwarePart, b"dataarea") => {
				let [name, size] = evt.find_attributes([b"name", b"size"])?;
				let name = name.unwrap_or_default().into();
				let size = parse(&size.unwrap_or_default())?;
				let data_area = SoftwareDataArea {
					name,
					size,
					assets: Default::default(),
				};
				self.current_data_areas.as_mut().unwrap().push(data_area);
				self.current_assets = Some(Vec::new());
				Some(Phase::SoftwarePartDataArea)
			}
			(Phase::SoftwarePartDataArea, b"rom") => {
				let [name, size, crc, sha1, status] =
					evt.find_attributes([b"name", b"size", b"crc", b"sha1", b"status"])?;
				let name = name.unwrap_or_default().into();
				let size = parse(&size.unwrap_or_default())?;
				let hash = AssetHash::from_hex_strings(crc.as_deref(), sha1.as_deref())?;
				let status = status
					.map(|x| x.parse::<AssetStatus>())
					.transpose()?
					.unwrap_or(AssetStatus::Good);
				let asset = SoftwareAsset {
					name,
					size,
					hash,
					status,
				};
				self.current_assets.as_mut().unwrap().push(asset);
				None
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(&mut self, text: Option<String>) -> Result<()> {
		match self.phase_stack.last().unwrap_or(&Phase::Root) {
			Phase::Software => {
				let software = Software::from(self.current_software.take().unwrap());
				self.software_list.software.push(software.into());
			}

			Phase::SoftwareDescription => {
				let description = text.unwrap().into();
				self.current_software.as_mut().unwrap().description = description;
			}
			Phase::SoftwareYear => {
				let year = text.unwrap().into();
				self.current_software.as_mut().unwrap().year = year;
			}
			Phase::SoftwarePublisher => {
				let publisher = text.unwrap().into();
				self.current_software.as_mut().unwrap().publisher = publisher;
			}
			Phase::SoftwarePart => {
				let data_areas = self.current_data_areas.take().unwrap().into();
				self.current_software
					.as_mut()
					.unwrap()
					.parts
					.last_mut()
					.unwrap()
					.data_areas = data_areas;
			}
			Phase::SoftwarePartDataArea => {
				let assets = self.current_assets.take().unwrap().into();
				self.current_data_areas.as_mut().unwrap().last_mut().unwrap().assets = assets;
			}
			_ => {}
		};
		Ok(())
	}
}

impl From<CurrentSoftware> for Software {
	fn from(value: CurrentSoftware) -> Self {
		Self {
			name: value.name,
			description: value.description,
			year: value.year,
			publisher: value.publisher,
			parts: value.parts.into(),
		}
	}
}

fn softlistxml_err(reader: &XmlReader<impl BufRead>, e: impl Into<Error>) -> Error {
	let message = format!(
		"Error parsing software list XML at position {}",
		reader.buffer_position()
	);
	e.into().context(message)
}

pub fn process_xml(reader: impl BufRead) -> Result<SoftwareList> {
	let mut state = State::new();
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

	use crate::assethash::AssetHash;
	use crate::info::AssetStatus;

	use super::process_xml;

	#[test_case(0, include_str!("test_data/softlist_coco_cart.xml"), ("coco_cart", "Tandy Radio Shack Color Computer cartridges", 112))]
	#[test_case(1, include_str!("test_data/softlist_msx1_cart.xml"), ("msx1_cart", "MSX1 cartridges", 1230))]
	pub fn general(_index: usize, xml: &str, expected: (&str, &str, usize)) {
		let reader = BufReader::new(xml.as_bytes());
		let software_list = process_xml(reader);
		let actual = software_list
			.as_ref()
			.map(|x| (x.name.as_ref(), x.description.as_ref(), x.software.len()))
			.map_err(|x| format!("{x}"));
		assert_eq!(Ok(expected), actual);
	}

	#[test_case(0, include_str!("test_data/softlist_coco_cart.xml"), "clowns", ("Clowns & Balloons", "1982", "Tandy"))]
	pub fn software(_index: usize, xml: &str, name: &str, expected: (&str, &str, &str)) {
		let reader = BufReader::new(xml.as_bytes());
		let software_list = process_xml(reader).unwrap();
		let software = software_list.software.iter().find(|x| x.name == name).unwrap().as_ref();
		let actual = (
			software.description.as_ref(),
			software.year.as_ref(),
			software.publisher.as_ref(),
		);
		assert_eq!(expected, actual);
	}

	#[allow(clippy::too_many_arguments)]
	#[test_case(0, include_str!("test_data/softlist_msx1_cart.xml"), "fsfd1", "fs_fd1.rom", 0x4000, Some("4c9b8214"), Some("8e3f6f08309f082a82be8298a66c9b90f2d34ad4"), AssetStatus::Good)]
	#[test_case(1, include_str!("test_data/softlist_msx1_cart.xml"), "vy0010", "27128q25-8.ic2", 0x4000, Some("164f5a6d"), Some("8924e3e11eb1c8c1edcb7efa63c26d2bdc142473"), AssetStatus::BadDump)]
	pub fn asset(
		_index: usize,
		xml: &str,
		software_name: &str,
		asset_name: &str,
		expected_size: u64,
		expected_crc: Option<&str>,
		expected_sha1: Option<&str>,
		expected_status: AssetStatus,
	) {
		let expected_hash = AssetHash::from_hex_strings(expected_crc, expected_sha1).unwrap();

		let reader = BufReader::new(xml.as_bytes());
		let software_list = process_xml(reader).unwrap();
		let asset = software_list
			.software
			.iter()
			.find(|x| x.name == software_name)
			.unwrap()
			.as_ref()
			.parts
			.iter()
			.flat_map(|p| &p.data_areas)
			.flat_map(|da| &da.assets)
			.find(|a| a.name == asset_name)
			.unwrap();

		assert_eq!(
			(expected_size, expected_hash, expected_status),
			(asset.size, asset.hash, asset.status)
		);
	}
}
