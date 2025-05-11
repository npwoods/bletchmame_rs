use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;
use std::time::Instant;

use anyhow::Error;
use byte_unit::Byte;

use crate::info::InfoDb;

#[derive(Clone, Copy, Debug)]
struct Metrics {
	byte_count: usize,
	elapsed_time: Duration,
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Error reading XML: {0:?}")]
	ReadingPath(std::io::Error),
	#[error("Error building InfoDb: {0:?}")]
	BuildingInfoDb(Error),
	#[error("InfoDb build process created corrupt database")]
	Validation(Vec<Error>),
}

pub fn info_db_from_xml_file(path: impl AsRef<Path>) -> ExitCode {
	match internal_info_db_from_xml_file(path) {
		Ok(metrics) => {
			let byte_count = Byte::from(metrics.byte_count);
			let (byte_count, unit) = byte_count.get_exact_unit(true);
			let elapsed_time = metrics.elapsed_time;
			println!("\x1B[KSuccess ({elapsed_time:?} elapsed; total size {byte_count} {unit})");
			ExitCode::SUCCESS
		}
		Err(e) => {
			println!("Error:  {e}");
			if let ThisError::Validation(errors) = e {
				for e in errors {
					println!("\t{e:?}");
				}
			}
			ExitCode::FAILURE
		}
	}
}

fn internal_info_db_from_xml_file(path: impl AsRef<Path>) -> std::result::Result<Metrics, ThisError> {
	let start_instant = Instant::now();
	let file = File::open(path).map_err(ThisError::ReadingPath)?;
	let mut reader = BufReader::new(file);

	let info_db = InfoDb::from_listxml_output(&mut reader, |machine_name| {
		print!("\x1B[KProcessing {machine_name}...\r");
		false
	})
	.map_err(ThisError::BuildingInfoDb)?;

	// cancellation should never happen
	let info_db = info_db.unwrap();

	// validate the results (which is not normally invoked on this path)
	info_db.validate().map_err(ThisError::Validation)?;

	// and return!
	let metrics = Metrics {
		byte_count: info_db.data_len(),
		elapsed_time: start_instant.elapsed(),
	};
	Ok(metrics)
}
