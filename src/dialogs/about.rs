use slint::CloseRequestResponse;
use tokio::sync::mpsc;

use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::ui::AboutDialog;

pub async fn dialog_about(modal_stack: ModalStack) {
	let modal = modal_stack.modal(|| AboutDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(());
		CloseRequestResponse::KeepWindowShown
	});

	// show the dialog
	modal
		.run(async {
			rx.recv().await.unwrap();
		})
		.await;
}
