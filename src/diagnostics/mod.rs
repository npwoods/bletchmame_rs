mod script;

use std::cell::RefCell;
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::io::stdin;
use std::io::stdout;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::Child;
use std::process::Command;
use std::process::ExitCode;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::spawn;
use std::time::Duration;
use std::time::Instant;

use anyhow::Error;
use anyhow::Result;
use byte_unit::Byte;
use console::Style;
use glob::GlobError;
use glob::PatternError;
use glob::glob;
use itertools::Itertools;
use throttle::Throttle;

use crate::console::EmitType;
use crate::diagnostics::script::Script;
use crate::info::InfoDb;
use crate::info::View;
use crate::runtime::command::MameCommand;
use crate::runtime::session::interact_with_mame;

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Error reading XML: {0:?}")]
	ReadingPath(std::io::Error),
	#[error("Error building InfoDb: {0:?}")]
	BuildingInfoDb(Error),
	#[error("InfoDb build process created corrupt database")]
	Validation(Vec<Error>),
	#[error("Error with glob: {0:?}")]
	Glob(Vec<GlobError>),
	#[error("Error with glob pattern: {0:?}")]
	GlobPattern(PatternError),
	#[error("Encountered MAME LUA error")]
	MameLuaError,
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
	let info_db_build_style = Style::new().reverse();

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

	println!("{}", info_db_build_style.apply_to(info_db.build()));
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

pub fn exercise_mame_tests(pattern: &str, args: &[impl AsRef<str>]) -> ExitCode {
	match internal_exercise_mame_tests(pattern, args) {
		Ok(()) => ExitCode::SUCCESS,
		Err(e) => {
			println!("Error:  {e}");
			ExitCode::FAILURE
		}
	}
}

fn internal_exercise_mame_tests(pattern: &str, command_line: &[impl AsRef<str>]) -> Result<()> {
	let (scripts, errors): (Vec<_>, Vec<_>) = glob(pattern).map_err(ThisError::GlobPattern)?.partition_result();
	if !errors.is_empty() {
		return Err(ThisError::Glob(errors).into());
	}

	for script in scripts.iter() {
		let mut mame_child = Command::new(command_line.first().unwrap().as_ref())
			.args(command_line[1..].iter().map(|s| s.as_ref()))
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()?;

		exercise_mame(&mut mame_child, script)?;

		let _ = mame_child.wait()?;
	}
	Ok(())
}

fn exercise_mame(mame_child: &mut Child, script_path: &Path) -> Result<()> {
	// styles
	let script_name_style = Style::new().bold().underlined();
	let stderr_style = Style::new().yellow();
	let bad_stderr_style = Style::new().red().bold();

	// parse the script
	let file = File::open(script_path)?;
	let reader = BufReader::new(file);
	let script = Script::parse(reader)?;
	let commands_iter = script.commands.iter();
	let commands_iter = RefCell::new(commands_iter);

	// we want to capture stderr
	let has_mame_lua_error = Arc::new(AtomicBool::new(false));
	let has_mame_lua_error_clone = has_mame_lua_error.clone();
	let mame_stderr = BufReader::new(mame_child.stderr.take().unwrap());
	let stderr_thread = spawn(move || {
		for line in mame_stderr.lines() {
			let line = line.unwrap();
			let is_mame_lua_error = line.starts_with("[LUA ERROR]");
			let style = if is_mame_lua_error {
				has_mame_lua_error_clone.store(true, Ordering::Relaxed);
				&bad_stderr_style
			} else {
				&stderr_style
			};
			println!("{}", style.apply_to(line));
		}
	});

	// print the name of the script we're running
	println!("{}", script_name_style.apply_to(script_path.to_string_lossy()));

	// and exercise MAME
	let receiver = move |_| {
		commands_iter
			.borrow_mut()
			.next()
			.map_or(MameCommand::exit(), |s| MameCommand::from_text(s.as_ref()))
	};
	let emit_console = |emit_type: EmitType, s: &str| {
		let style = emit_type.style();
		println!("{}", style.apply_to(s));
	};
	let event_callback = |_event| {};
	interact_with_mame(mame_child, &receiver, &emit_console, &event_callback)?;

	// wait for stderr to complete
	stderr_thread.join().unwrap();

	// have we seen any MAME Lua errors?
	if has_mame_lua_error.load(Ordering::Relaxed) {
		return Err(ThisError::MameLuaError.into());
	}

	// and we're done!
	Ok(())
}
