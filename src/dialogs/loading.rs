use std::io::BufReader;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::SharedString;
use slint::Weak;
use throttle::Throttle;
use tokio::task::spawn_blocking;

use crate::guiutils::modal::Modal;
use crate::info::InfoDb;
use crate::platform::CommandExt;
use crate::ui::LoadingDialog;

/// Presents a modal dialog for loading InfoDb from `mame -listxml`
pub async fn dialog_load_mame_info(
	parent: Weak<impl ComponentHandle + 'static>,
	mame_executable: &str,
) -> Option<InfoDb> {
	// sanity checks
	assert!(!mame_executable.is_empty());

	// present the dialog
	let modal = Modal::new(&parent.unwrap(), || LoadingDialog::new().unwrap());
	modal
		.dialog()
		.set_current_status("Retrieving machine info from MAME...".into());
	modal.dialog().show().unwrap();

	// then launch the process
	let process = Command::new(mame_executable)
		.arg("-listxml")
		.arg("-nodtd")
		.stdout(Stdio::piped())
		.create_no_window(true)
		.spawn()
		.unwrap();

	// communicating that we're cancelling is awkward; hence this Arc
	let cancelled = Arc::new(AtomicBool::new(false));

	// set up a close requested handler
	let cancelled_clone = cancelled.clone();
	modal.window().on_close_requested(move || {
		cancelled_clone.store(true, Ordering::Relaxed);
		CloseRequestResponse::HideWindow
	});

	// and with that out of the way, launch the thread
	let dialog_weak = modal.dialog().as_weak();
	let fut = spawn_blocking(move || load_mame_info_thread_proc(dialog_weak, process, cancelled));
	modal.run(fut).await.unwrap()
}

/// worker thread for loading MAME info
fn load_mame_info_thread_proc(
	dialog_weak: Weak<LoadingDialog>,
	mut process: Child,
	cancelled: Arc<AtomicBool>,
) -> Option<InfoDb> {
	// progress messages need to be throttled
	let mut throttle = Throttle::new(Duration::from_millis(100), 1);

	// access the MAME process stdout (which is input to us)
	let input = process.stdout.as_mut().unwrap();

	// prepare a callback for the InfoDB loading code
	let dialog_weak_clone = dialog_weak.clone();
	let info_db_callback = move |machine_description: &str| {
		// do we need to update
		if throttle.accept().is_ok() {
			// we do need to update

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

	// and close out the process (we don't want it to zombie)
	let _ = process.wait();

	// and return!
	db
}
