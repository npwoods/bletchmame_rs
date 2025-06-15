use std::sync::Arc;

use more_asserts::assert_ge;
use slint::CloseRequestResponse;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;

use crate::appcommand::AppCommand;
use crate::dialogs::SingleResult;
use crate::guiutils::modal::ModalStack;
use crate::runtime::command::SeqType;
use crate::ui::InputSelectMultipleDialog;

pub async fn dialog_input_select_multiple(
	modal_stack: ModalStack,
	selections: impl AsRef<[(String, Vec<(Arc<str>, u32, SeqType, String)>)]>,
) -> Option<AppCommand> {
	// the basics
	let selections = selections.as_ref();
	assert_ge!(selections.len(), 2);

	// set up the modal
	let modal = modal_stack.modal(|| InputSelectMultipleDialog::new().unwrap());
	let single_result = SingleResult::default();

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(false);
		CloseRequestResponse::KeepWindowShown
	});

	// set up the "cancel" button
	let signaller = single_result.signaller();
	modal.dialog().on_cancel_clicked(move || {
		signaller.signal(false);
	});

	// set up the "ok" button
	let signaller = single_result.signaller();
	modal.dialog().on_ok_clicked(move || {
		signaller.signal(true);
	});

	// set up entries
	let entries = selections
		.iter()
		.map(|(title, _)| SharedString::from(title))
		.collect::<Vec<_>>();
	let entries = VecModel::from(entries);
	let entries = ModelRc::new(entries);
	modal.dialog().set_entries(entries);

	// present the modal dialog
	let result = modal.run(async { single_result.wait().await }).await;

	// return the result
	result.then(|| todo!())
}
