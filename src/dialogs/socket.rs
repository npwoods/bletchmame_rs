use slint::CloseRequestResponse;
use slint::ComponentHandle;
use tokio::sync::mpsc;

use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::ui::ConnectToSocketDialog;

pub async fn dialog_connect_to_socket(modal_stack: ModalStack) -> Option<(String, u16)> {
	// prepare the dialog
	let modal = modal_stack.modal(|| ConnectToSocketDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set up the accepted handler (when "OK" is clicked)
	let tx_clone = tx.clone();
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_accepted(move || {
		let dialog = dialog_weak.unwrap();
		let result = get_results(&dialog).unwrap();
		tx_clone.signal(Some(result));
	});

	// set up the cancelled handler (when "Cancel" is clicked)
	let tx_clone = tx.clone();
	modal.dialog().on_cancelled(move || {
		tx_clone.signal(None);
	});

	// set up the changed handler
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_changed(move || {
		update_can_accept(&dialog_weak.unwrap());
	});

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// set up defaults
	modal.dialog().set_host_text("localhost".into());
	modal.dialog().set_port_text("12345".into());
	update_can_accept(modal.dialog());

	// present the modal dialog
	modal.run(async { rx.recv().await.unwrap() }).await
}

fn update_can_accept(dialog: &ConnectToSocketDialog) {
	let is_enabled = get_results(dialog).is_some();
	dialog.set_can_accept(is_enabled);
}

fn get_results(dialog: &ConnectToSocketDialog) -> Option<(String, u16)> {
	let host_text = dialog.get_host_text();
	let port_text = dialog.get_port_text();
	let port = port_text.parse().ok()?;
	let is_valid = hostname_validator::is_valid(&host_text);
	is_valid.then(|| (host_text.into(), port))
}
