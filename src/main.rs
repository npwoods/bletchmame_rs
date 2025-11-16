#![cfg_attr(not(test), windows_subsystem = "windows")]
mod action;
mod appstate;
mod appwindow;
mod assethash;
mod audit;
mod backend;
mod canceller;
mod channel;
mod collections;
mod console;
mod debugstr;
mod devimageconfig;
mod diagnostics;
mod dialogs;
mod guiutils;
mod history;
mod history_xml;
mod icon;
mod imagedesc;
mod importmameini;
mod info;
mod job;
mod mconfig;
mod models;
mod parse;
mod platform;
mod prefs;
mod runtime;
mod selection;
mod snapview;
mod software;
mod status;
mod threadlocalbubble;
mod version;
mod xml;

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use appwindow::AppWindowing;
use dirs::config_local_dir;
use slint::ComponentHandle;
use slint::run_event_loop;
use slint::spawn_local;
use structopt::StructOpt;
use tracing_subscriber::EnvFilter;

use crate::appwindow::AppArgs;
use crate::backend::BackendRuntime;
use crate::backend::SlintBackend;
use crate::diagnostics::info_db_from_xml_file;
use crate::platform::platform_init;
use crate::runtime::MameStderr;
use crate::ui::AppWindow;

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

	#[cfg(target_os = "windows")]
	#[structopt(long)]
	echo_console: Option<String>,

	#[cfg_attr(feature = "diagnostics", structopt(long))]
	process_listxml: bool,

	#[cfg_attr(feature = "diagnostics", structopt(long, parse(from_os_str)))]
	process_listxml_file: Option<PathBuf>,

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
	let process_listxml = match (opts.process_listxml, opts.process_listxml_file.as_deref()) {
		(false, None) => None,
		(true, None) => Some(None),
		(false, Some(path)) => Some(Some(path)),
		(true, Some(_)) => panic!("Cannot specify --process-listxml and --process-listxml-file simultaneously"),
	};
	if let Some(path) = process_listxml {
		return info_db_from_xml_file(path);
	}

	// echo console?
	#[cfg(target_os = "windows")]
	if let Some(pipe_name) = opts.echo_console.as_deref() {
		return if crate::platform::win_echo_console_main(pipe_name).is_ok() {
			ExitCode::SUCCESS
		} else {
			ExitCode::FAILURE
		};
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
	let backend_runtime = BackendRuntime::new(backend_type).expect("slint backend setup failed");

	// run the application window...
	let args = AppArgs {
		prefs_path,
		mame_stderr,
		mame_windowing,
		backend_runtime,
	};
	let app_window = AppWindow::new().expect("Failed to create main window");
	let app_window = Rc::new(app_window);
	let app_window_clone = app_window.clone();
	let fut = async move {
		appwindow::start(&app_window_clone, args).await;
	};
	spawn_local(fut).unwrap();

	// ...and run run run!
	run_event_loop().unwrap();
	let _ = app_window.hide();
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
