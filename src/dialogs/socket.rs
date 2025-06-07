use slint::CloseRequestResponse;
use slint::ComponentHandle;

use crate::dialogs::SingleResult;
use crate::guiutils::modal::ModalStack;
use crate::ui::ConnectToSocketDialog;

pub async fn dialog_connect_to_socket(modal_stack: ModalStack) -> Option<(String, u16)> {
	// prepare the dialog
	let modal = modal_stack.modal(|| ConnectToSocketDialog::new().unwrap());
	let single_result = SingleResult::default();

	// set up the accepted handler (when "OK" is clicked)
	let signaller = single_result.signaller();
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_accepted(move || {
		let dialog = dialog_weak.unwrap();
		let result = get_results(&dialog).unwrap();
		signaller.signal(Some(result));
	});

	// set up the cancelled handler (when "Cancel" is clicked)
	let signaller = single_result.signaller();
	modal.dialog().on_cancelled(move || {
		signaller.signal(None);
	});

	// set up the changed handler
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_changed(move || {
		update_can_accept(&dialog_weak.unwrap());
	});

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// set up defaults
	modal.dialog().set_host_text("localhost".into());
	modal.dialog().set_port_text("12345".into());
	update_can_accept(modal.dialog());

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await
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
