use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::io::stdin;
use std::io::stdout;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;
use std::time::Instant;

use anyhow::Error;
use byte_unit::Byte;
use console::style;
use throttle::Throttle;

use crate::info::InfoDb;
use crate::info::View;

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Error reading XML: {0:?}")]
	ReadingPath(std::io::Error),
	#[error("Error building InfoDb: {0:?}")]
	BuildingInfoDb(Error),
	#[error("InfoDb build process created corrupt database")]
	Validation(Vec<Error>),
}

pub fn info_db_from_xml_file(path: Option<impl AsRef<Path>>) -> ExitCode {
	match internal_info_db_from_xml_file(path) {
		Ok((info_db, elapsed_time)) => {
			println!("\x1B[KInfoDB Processing Succeeded!");
			println!();
			print_stats(&info_db, elapsed_time);
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

fn print_stats(info_db: &InfoDb, elapsed_time: Duration) {
	let count_width = 8;
	let (total_size, total_size_unit) = Byte::from(info_db.data_len()).get_exact_unit(true);
	let (strings_size, strings_size_unit) = Byte::from(info_db.strings_len()).get_exact_unit(true);

	// these are all of the entry counts and associated labels
	let entry_counts = [
		("Machines", info_db.machines().len()),
		("ROMs", info_db.roms().len()),
		("Disks", info_db.disks().len()),
		("Samples", info_db.samples().len()),
		("BIOS Sets", info_db.biossets().len()),
		("Chips", info_db.chips().len()),
		("Configurations", info_db.configurations().len()),
		("Configuration Settings", info_db.configuration_settings().len()),
		(
			"Configuration Setting Conditions",
			info_db.configuration_setting_conditions().len(),
		),
		("Devices", info_db.devices().len()),
		("Device Refs", info_db.device_refs().len()),
		("Slots", info_db.slots().len()),
		("Slots Options", info_db.slot_options().len()),
		("Software Lists", info_db.software_lists().len()),
		(
			"Machine --> Software List Indexes",
			info_db.software_list_machine_indexes().len(),
		),
		(
			"Software List --> Machine Indexes",
			info_db.machine_software_lists().len(),
		),
		("RAM Options", info_db.ram_options().len()),
	];

	// figure out how wide the largest label is
	let max_label_width = entry_counts.iter().map(|(label, _)| label.len()).max().unwrap();

	println!("{}", style(info_db.build()).reverse());
	for (label, count) in entry_counts {
		println!("{:<max_label_width$}: {:>count_width$}", label, count);
	}
	println!(
		"{:<max_label_width$}: {:>count_width$} {}",
		"String Table Length", strings_size, strings_size_unit
	);
	println!();
	println!(
		"{:<max_label_width$}: {:>count_width$} {}",
		"Total Size", total_size, total_size_unit
	);
	println!(
		"{:<max_label_width$}: {:>count_width$} secs",
		"Elapsed Time",
		elapsed_time.as_millis() as f32 / 1000.0
	);
}

fn internal_info_db_from_xml_file(
	path: Option<impl AsRef<Path>>,
) -> std::result::Result<(InfoDb, Duration), ThisError> {
	let start_instant = Instant::now();
	let mut throttle = Throttle::new(Duration::from_millis(30), 1);
	let file = if let Some(path) = path {
		let file = File::open(path).map_err(ThisError::ReadingPath)?;
		Box::new(file) as Box<dyn Read>
	} else {
		Box::new(stdin()) as Box<dyn Read>
	};
	let mut reader = BufReader::new(file);

	let info_db = InfoDb::from_listxml_output(&mut reader, |machine_name| {
		if throttle.accept().is_ok() {
			print!("\x1B[KProcessing {machine_name}...\r");
			let _ = stdout().flush();
		}
		ControlFlow::Continue(())
	})
	.map_err(ThisError::BuildingInfoDb)?;

	// cancellation should never happen
	let info_db = info_db.unwrap();

	// validate the results (which is not normally invoked on this path)
	info_db.validate().map_err(ThisError::Validation)?;

	// and return!
	Ok((info_db, start_instant.elapsed()))
}
