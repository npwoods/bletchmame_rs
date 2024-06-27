mod appwindow;
mod error;
mod guiutils;
mod info;
mod loading;
mod models;
mod prefs;
mod threadlocalbubble;

use slint::ComponentHandle;

use crate::guiutils::init_gui_utils;

mod ui {
	slint::include_modules!();
}

type Error = crate::error::Error;
type Result<T> = std::result::Result<T, Box<crate::error::Error>>;

fn main() {
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
