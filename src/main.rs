mod appcommand;
mod appwindow;
mod diagnostics;
mod dialogs;
mod error;
mod guiutils;
mod info;
mod models;
mod prefs;
mod software;
mod threadlocalbubble;
mod xml;

use std::path::PathBuf;

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
	#[cfg(feature = "diagnostics")]
	#[structopt(long, parse(from_os_str))]
	process_xml: Option<PathBuf>,
}

impl Opt {
	#[cfg(feature = "diagnostics")]
	pub fn process_xml_path(&self) -> Option<&PathBuf> {
		self.process_xml.as_ref()
	}

	#[cfg(not(feature = "diagnostics"))]
	pub fn process_xml_path(&self) -> Option<&PathBuf> {
		None
	}
}

fn main() {
	let opts = Opt::from_args();

	// are we doing diagnostics
	if let Some(path) = opts.process_xml_path() {
		info_db_from_xml_file(&path);
		return;
	}

	// set up the tokio runtime
	let tokio_runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
	let _guard = tokio_runtime.enter();

	// initialize our GUI utility code that will hopefully go away as Slint improves
	init_gui_utils();

	// create the application winodw...
	let app_window = appwindow::create();

	// ...and run run run!
	app_window.run().unwrap();
}
