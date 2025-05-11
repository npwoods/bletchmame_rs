#![cfg_attr(not(test), windows_subsystem = "windows")]
mod appcommand;
mod appstate;
mod appwindow;
mod channel;
mod childwindow;
mod collections;
mod debugstr;
mod devimageconfig;
mod diagnostics;
mod dialogs;
mod guiutils;
mod history;
mod icon;
mod info;
mod job;
mod mconfig;
mod models;
mod parse;
mod platform;
mod prefs;
mod runtime;
mod selection;
mod software;
mod status;
mod threadlocalbubble;
mod version;
mod xml;

use std::path::PathBuf;
use std::process::ExitCode;

use appwindow::AppWindowing;
use dirs::config_local_dir;
use slint::ComponentHandle;
use structopt::StructOpt;
use tracing_subscriber::EnvFilter;

use crate::appwindow::AppArgs;
use crate::diagnostics::info_db_from_xml_file;
use crate::guiutils::SlintBackend;
use crate::guiutils::init_slint_backend;
use crate::platform::platform_init;
use crate::runtime::MameStderr;

mod ui {
	slint::include_modules!();
}

#[derive(StructOpt, Debug)]
#[structopt(name = "basic")]
struct Opt {
	#[structopt(long, parse(from_os_str))]
	prefs_path: Option<PathBuf>,

	#[structopt(long)]
	mame_windowing: Option<AppWindowing>,

	#[cfg_attr(feature = "slint-qt-backend", structopt(long))]
	slint_backend: Option<SlintBackend>,

	#[cfg_attr(feature = "diagnostics", structopt(long, parse(from_os_str)))]
	process_xml: Option<PathBuf>,

	#[cfg_attr(feature = "diagnostics", structopt(long))]
	log: Option<String>,

	#[cfg_attr(feature = "diagnostics", structopt(long))]
	no_capture_mame_stderr: bool,
}

fn main() -> ExitCode {
	// platform-specific stuff
	let _platform_specific = platform_init().unwrap();

	// get the command line arguments
	let opts = Opt::from_args();

	// set up logging
	let log = opts.log.or_else(|| std::env::var("RUST_ENV").ok());
	if let Some(log) = log {
		tracing_subscriber::fmt().with_env_filter(EnvFilter::new(log)).init();
	}

	// are we doing diagnostics
	if let Some(path) = opts.process_xml {
		return info_db_from_xml_file(path);
	}

	// identify the preferences directory
	let prefs_path = opts.prefs_path.unwrap_or_else(|| {
		let mut path = config_local_dir().unwrap_or_default();
		path.push("BletchMAME");
		path
	});

	// are we supposed to capture MAME's stderr? we almost always do, except when debugging
	let mame_stderr = if opts.no_capture_mame_stderr {
		MameStderr::Inherit
	} else {
		MameStderr::Capture
	};

	// windowing?
	let mame_windowing = opts.mame_windowing.unwrap_or_default();

	// set up the tokio runtime
	let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
		.enable_time()
		.build()
		.unwrap();
	let _guard = tokio_runtime.enter();

	// set up the Slint back end
	let backend_type = opts.slint_backend.unwrap_or_default();
	init_slint_backend(backend_type).expect("slint backend setup failed");

	// create the application window...
	let args = AppArgs {
		prefs_path,
		mame_stderr,
		mame_windowing,
	};
	let app_window = appwindow::create(args);

	// ...and run run run!
	app_window.run().unwrap();
	ExitCode::SUCCESS
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
