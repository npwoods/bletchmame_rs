use std::io::BufRead;

use anyhow::Error;
use anyhow::Result;
use tracing::event;
use tracing::Level;

use crate::parse::parse_mame_bool;
use crate::status::Update;
use crate::status::UpdateRunning;
use crate::version::MameVersion;
use crate::xml::XmlElement;
use crate::xml::XmlEvent;
use crate::xml::XmlReader;

const LOG: Level = Level::TRACE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
	Root,
	Status,
}

#[derive(Debug)]
struct State {
	phase_stack: Vec<Phase>,
	build: Option<MameVersion>,
	running: UpdateRunning,
}

impl State {
	pub fn handle_start(&mut self, evt: XmlElement<'_>) -> Result<Option<Phase>> {
		let phase = self.phase_stack.last().unwrap_or(&Phase::Root);
		let new_phase = match (phase, evt.name().as_ref()) {
			(Phase::Root, b"status") => {
				let [romname, is_paused, app_build] = evt.find_attributes([b"romname", b"paused", b"app_build"])?;
				let machine_name = romname.unwrap_or_default().to_string();
				let is_paused = is_paused.map(|x| parse_mame_bool(x.as_ref())).transpose()?;
				event!(
					LOG,
					"status State::handle_start(): machine_name={} is_paused={:?}",
					machine_name,
					is_paused
				);

				self.build = app_build.map(MameVersion::from);
				self.running.machine_name = machine_name;
				self.running.is_paused = is_paused;
				Some(Phase::Status)
			}
			(Phase::Status, b"video") => {
				let [throttled, throttle_rate] = evt.find_attributes([b"throttled", b"throttle_rate"])?;
				let throttled = throttled.map(parse_mame_bool).transpose()?;
				let throttle_rate = throttle_rate.map(|x| x.parse::<f32>()).transpose()?;

				event!(
					LOG,
					"status State::handle_start(): throttled={:?} throttle_rate={:?}",
					throttled,
					throttle_rate
				);

				self.running.is_throttled = throttled.or(self.running.is_throttled);
				self.running.throttle_rate = throttle_rate.or(self.running.throttle_rate);
				None
			}
			(Phase::Status, b"sound") => {
				let [attenuation] = evt.find_attributes([b"attenuation"])?;
				let attenuation = attenuation.map(|x| x.parse::<i32>()).transpose()?;
				self.running.sound_attenuation = attenuation.or(self.running.sound_attenuation);
				None
			}
			_ => None,
		};
		Ok(new_phase)
	}

	pub fn handle_end(&mut self, _text: Option<String>) -> Result<()> {
		Ok(())
	}
}

pub fn parse_update(reader: impl BufRead) -> Result<Update> {
	let mut reader = XmlReader::from_reader(reader, false);
	let mut buf = Vec::with_capacity(1024);
	let mut state = State {
		phase_stack: Vec::with_capacity(32),
		build: None,
		running: UpdateRunning::default(),
	};

	while let Some(evt) = reader.next(&mut buf).map_err(|e| statusxml_err(&reader, e))? {
		match evt {
			XmlEvent::Start(evt) => {
				let new_phase = state.handle_start(evt).map_err(|e| statusxml_err(&reader, e))?;
				if let Some(new_phase) = new_phase {
					state.phase_stack.push(new_phase);
				} else {
					reader.start_unknown_tag();
				}
			}

			XmlEvent::End(s) => {
				state.handle_end(s).map_err(|e| statusxml_err(&reader, e))?;
				state.phase_stack.pop().unwrap();
			}

			XmlEvent::Null => {} // meh
		}
	}

	let running = (!state.running.machine_name.is_empty()).then_some(state.running);
	let result = Update {
		running,
		build: state.build,
	};
	Ok(result)
}

fn statusxml_err(reader: &XmlReader<impl BufRead>, e: impl Into<Error>) -> Error {
	let message = format!("Error parsing status XML at position {}", reader.buffer_position());
	e.into().context(message)
}

#[cfg(test)]
mod test {
	use std::io::BufReader;

	use assert_matches::assert_matches;
	use test_case::test_case;

	use super::parse_update;

	#[test_case(0, include_str!("test_data/status_mame0226_coco2b_1.xml"))]
	#[test_case(1, include_str!("test_data/status_mame0227_coco2b_1.xml"))]
	#[test_case(2, include_str!("test_data/status_mame0270_1.xml"))]
	#[test_case(3, include_str!("test_data/status_mame0270_coco2b_1.xml"))]
	#[test_case(4, include_str!("test_data/status_mame0270_coco2b_2.xml"))]
	#[test_case(5, include_str!("test_data/status_mame0270_coco2b_3.xml"))]
	#[test_case(6, include_str!("test_data/status_mame0270_coco2b_4.xml"))]
	fn general(_index: usize, xml: &str) {
		let reader = BufReader::new(xml.as_bytes());
		let result = parse_update(reader);
		assert_matches!(result, Ok(_));
	}

	#[test_case(0, include_str!("test_data/status_mame0226_coco2b_1.xml"), Some(true), Some(1.0))]
	#[test_case(1, include_str!("test_data/status_mame0227_coco2b_1.xml"), Some(true), Some(1.0))]
	#[test_case(2, include_str!("test_data/status_mame0270_coco2b_1.xml"), Some(true), Some(1.0))]
	#[test_case(3, include_str!("test_data/status_mame0270_coco2b_2.xml"), Some(true), Some(1.0))]
	#[test_case(4, include_str!("test_data/status_mame0270_coco2b_3.xml"), None, None)]
	#[test_case(5, include_str!("test_data/status_mame0270_coco2b_4.xml"), Some(false), Some(3.0))]
	fn throttling(_index: usize, xml: &str, expected_is_throttled: Option<bool>, expected_throttle_rate: Option<f32>) {
		let expected = (expected_is_throttled, expected_throttle_rate);

		let reader = BufReader::new(xml.as_bytes());
		let running = parse_update(reader).unwrap().running.unwrap();
		let actual = (running.is_throttled, running.throttle_rate);
		assert_eq!(expected, actual);
	}
}
