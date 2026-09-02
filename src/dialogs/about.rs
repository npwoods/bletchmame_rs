use slint::CloseRequestResponse;
use tokio::sync::mpsc;
use tracing::error;

use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::ui::AboutDialog;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn dialog_about(modal_stack: ModalStack) {
	let modal = modal_stack.modal(|| AboutDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set the version
	let version = if VERSION.starts_with('0') {
		"(development version)".into()
	} else {
		format!("verssion {VERSION}").into()
	};
	modal.dialog().set_version(version);

	// set up the link-clicked callback
	modal.dialog().on_link_clicked(|link| {
		if let Err(e) = open::that(&link) {
			error!("Failed to open link: {}", e);
		}
	});

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
