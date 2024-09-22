#![cfg_attr(not(test), windows_subsystem = "windows")]
mod appcommand;
mod appwindow;
mod collections;
mod diagnostics;
mod dialogs;
mod error;
mod guiutils;
mod history;
mod info;
mod models;
mod prefs;
mod selection;
mod software;
mod threadlocalbubble;
mod xml;

use std::path::PathBuf;
use tracing::Level;
use winapi::um::wincon::AttachConsole;
use winapi::um::wincon::ATTACH_PARENT_PROCESS;

use dirs::config_local_dir;
use slint::ComponentHandle;
use structopt::StructOpt;

use crate::diagnostics::info_db_from_xml_file;
use crate::guiutils::init_gui_utils;

mod ui {
	slint::include_modules!();
}

type Error = crate::error::Error;
type Result<T> = std::result::Result<T, Box<crate::error::Error>>;

#[derive(StructOpt, Debug)]
#[structopt(name = "basic")]
struct Opt {
	#[cfg_attr(not(feature = "no-diagnostics"), structopt(long, parse(from_os_str)))]
	process_xml: Option<PathBuf>,

	#[structopt(long, parse(from_os_str))]
	prefs_path: Option<PathBuf>,

	#[cfg_attr(not(feature = "no-diagnostics"), structopt(long))]
	log_level: Option<Level>,
}

fn main() {
	// on Windows, attach to the parent's console - debugging is hell if we don't do this
	#[cfg(target_os = "windows")]
	unsafe {
		AttachConsole(ATTACH_PARENT_PROCESS);
	}

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

	// set up the tokio runtime
	let tokio_runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
	let _guard = tokio_runtime.enter();

	// initialize our GUI utility code that will hopefully go away as Slint improves
	init_gui_utils();

	// create the application winodw...
	let app_window = appwindow::create(prefs_path);

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
