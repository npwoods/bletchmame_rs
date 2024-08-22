use std::io::BufReader;
use std::io::Read;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::SharedString;
use slint::Weak;
use tokio::task::spawn_blocking;

use crate::guiutils::windowing::with_modal_parent;
use crate::info::InfoDb;
use crate::ui::LoadingDialog;

const UPDATE_INTERVAL: Duration = Duration::from_millis(250);

/// Presents a modal dialog for loading InfoDb from `mame -listxml`
pub async fn dialog_load_mame_info(
	parent: Weak<impl ComponentHandle + 'static>,
	mame_executable: &str,
) -> Option<InfoDb> {
	// sanity checks
	assert!(!mame_executable.is_empty());

	// present the dialog
	let dialog = with_modal_parent(&parent.unwrap(), || LoadingDialog::new().unwrap());
	dialog.set_current_status("Retrieving machine info from MAME...".into());
	dialog.show().unwrap();

	// then launch the process
	let process = Command::new(mame_executable)
		.arg("-listxml")
		.arg("-nodtd")
		.stdout(Stdio::piped())
		.spawn()
		.unwrap();

	// communicating that we're cancelling is awkward; hence this Arc
	let cancelled = Arc::new(AtomicBool::new(false));

	// set up a close requested handler
	let cancelled_clone = cancelled.clone();
	dialog.window().on_close_requested(move || {
		cancelled_clone.store(true, Ordering::Relaxed);
		CloseRequestResponse::HideWindow
	});

	// and with that out of the way, launch the thread
	let dialog_weak = dialog.as_weak();
	spawn_blocking(move || load_mame_info_thread_proc(dialog_weak, process.stdout.unwrap(), cancelled))
		.await
		.unwrap()
}

/// worker thread for loading MAME info
fn load_mame_info_thread_proc(
	dialog_weak: Weak<LoadingDialog>,
	input: impl Read,
	cancelled: Arc<AtomicBool>,
) -> Option<InfoDb> {
	let mut last_updated_time = None;

	// prepare a callback for the InfoDB loading code
	let dialog_weak_clone = dialog_weak.clone();
	let info_db_callback = move |machine_description: &str| {
		// do we need to update
		if last_updated_time
			.map(|x: Instant| x.elapsed() < UPDATE_INTERVAL)
			.unwrap_or(true)
		{
			// we do need to update
			last_updated_time = Some(Instant::now());

			// issue the request to update the machine on the dialog, and poll for
			// cancellation while we're at it
			let machine_description = SharedString::from(machine_description);
			let cancelled_clone = cancelled.clone();
			dialog_weak_clone
				.upgrade_in_event_loop(move |dialog| {
					dialog.set_current_status(machine_description);
					cancelled_clone.store(dialog.get_cancelled(), Ordering::Relaxed);
				})
				.unwrap();
		};

		// have we cancelled?
		cancelled.load(Ordering::Relaxed)
	};

	// process the InfoDB output
	let reader = BufReader::new(input);
	let db = InfoDb::from_listxml_output(reader, info_db_callback).unwrap();

	// now that we're done, close out the dialog
	dialog_weak
		.upgrade_in_event_loop(|dialog| dialog.hide().unwrap())
		.unwrap();
	db
}
