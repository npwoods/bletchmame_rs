use slint::CloseRequestResponse;
use slint::ComponentHandle;
use tokio::sync::mpsc;

use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::ui::StopWarningDialog;

pub struct StopWarningResult {
	pub stop: bool,
	pub show_warning: Option<bool>,
}

pub async fn dialog_stop_warning(modal_stack: ModalStack, show_warning: bool) -> StopWarningResult {
	let modal = modal_stack.modal(|| StopWarningDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set up the "yes" button
	let tx_clone = tx.clone();
	modal.dialog().on_yes_clicked(move || {
		tx_clone.signal(Some(true));
	});

	// set up the "no" button
	let tx_clone = tx.clone();
	modal.dialog().on_no_clicked(move || {
		tx_clone.signal(Some(false));
	});

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// set up the "show warning" checkbox
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().set_show_warning_checked(show_warning);

	// show the dialog and wait for completion
	modal
		.run(async {
			let result = rx.recv().await.unwrap();
			let show_warning = result
				.is_some()
				.then(|| dialog_weak.unwrap().get_show_warning_checked());
			let stop = result.unwrap_or(false);
			StopWarningResult { stop, show_warning }
		})
		.await
}
