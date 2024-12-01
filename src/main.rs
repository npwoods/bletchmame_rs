#![cfg_attr(not(test), windows_subsystem = "windows")]
mod appcommand;
mod appwindow;
mod collections;
mod debugstr;
mod diagnostics;
mod dialogs;
mod guiutils;
mod history;
mod icon;
mod info;
mod models;
mod prefs;
mod runtime;
mod selection;
mod software;
mod status;
mod threadlocalbubble;
mod version;
mod xml;

use std::path::PathBuf;

use dirs::config_local_dir;
use slint::ComponentHandle;
use structopt::StructOpt;
use tracing::Level;
use win32job::Job;
use win32job::JobError;
use winapi::um::wincon::AttachConsole;
use winapi::um::wincon::ATTACH_PARENT_PROCESS;

use crate::diagnostics::info_db_from_xml_file;
use crate::guiutils::init_gui_utils;
use crate::runtime::MameStderr;

mod ui {
	slint::include_modules!();
}

#[derive(StructOpt, Debug)]
#[structopt(name = "basic")]
struct Opt {
	#[structopt(long, parse(from_os_str))]
	prefs_path: Option<PathBuf>,

	#[cfg_attr(feature = "diagnostics", structopt(long, parse(from_os_str)))]
	process_xml: Option<PathBuf>,

	#[cfg_attr(feature = "diagnostics", structopt(long))]
	log_level: Option<Level>,

	#[cfg_attr(feature = "diagnostics", structopt(long))]
	no_capture_mame_stderr: bool,
}

#[cfg(target_os = "windows")]
fn setup_win32_job() -> std::result::Result<Job, JobError> {
	let job = Job::create()?;
	let mut info = job.query_extended_limit_info()?;
	info.limit_kill_on_job_close();
	job.set_extended_limit_info(&info)?;
	job.assign_current_process()?;
	Ok(job)
}

fn main() {
	// stuff specific to Windows
	#[cfg(target_os = "windows")]
	let _job = {
		// attach to the parent's console - debugging is hell if we don't do this
		unsafe {
			AttachConsole(ATTACH_PARENT_PROCESS);
		}
		setup_win32_job().unwrap()
	};

	// get the command line arguments
	let opts = Opt::from_args();

	// set up logging
	tracing_subscriber::fmt()
		.with_max_level(opts.log_level.unwrap_or(Level::INFO))
		.with_target(false)
		.init();

	// are we doing diagnostics
	if let Some(path) = opts.process_xml {
		info_db_from_xml_file(path);
		return;
	}

	// identify the preferences directory
	let prefs_path = opts.prefs_path.or_else(|| {
		let mut path = config_local_dir();
		if let Some(path) = &mut path {
			path.push("BletchMAME");
		}
		path
	});

	// are we supposed to capture MAME's stderr? we almost always do, except when debugging
	let mame_stderr = if opts.no_capture_mame_stderr {
		MameStderr::Inherit
	} else {
		MameStderr::Capture
	};

	// set up the tokio runtime
	let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
		.enable_time()
		.build()
		.unwrap();
	let _guard = tokio_runtime.enter();

	// initialize our GUI utility code that will hopefully go away as Slint improves
	init_gui_utils();

	// create the application winodw...
	let app_window = appwindow::create(prefs_path, mame_stderr);

	// ...and run run run!
	app_window.run().unwrap();
}

#[cfg(test)]
mod test {
	use assert_matches::assert_matches;
	use structopt::StructOpt;

	use super::Opt;

	#[test]
	fn opts_from_args() {
		let empty_args = Vec::<&str>::new();
		let attrs = Opt::from_iter_safe(empty_args.iter());
		assert_matches!(attrs, Ok(_));
	}
}
