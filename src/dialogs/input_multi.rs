use std::sync::Arc;

use slint::CloseRequestResponse;

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::dialogs::SingleResult;
use crate::guiutils::modal::ModalStack;
use crate::status::Input;
use crate::status::InputDeviceClass;
use crate::status::Status;
use crate::ui::InputDialogEntry;
use crate::ui::InputMultiDialog;

pub async fn dialog_input_multi(
	modal_stack: ModalStack,
	x_input: Option<(impl AsRef<str>, u32)>,
	y_input: Option<(impl AsRef<str>, u32)>,
	inputs: Arc<[Input]>,
	_input_device_classes: Arc<[InputDeviceClass]>,
	_status_changed_channel: Channel<Status>,
	_invoke_command: impl Fn(AppCommand) + Clone + 'static,
) {
	// look up the inputs
	let x_input = x_input
		.map(|(port_tag, mask)| {
			let target = (port_tag.as_ref(), mask);
			inputs.as_ref().iter().find(|x| target == (x.port_tag.as_ref(), mask))
		})
		.unwrap();
	let y_input = y_input
		.map(|(port_tag, mask)| {
			let target = (port_tag.as_ref(), mask);
			inputs.as_ref().iter().find(|x| target == (x.port_tag.as_ref(), mask))
		})
		.unwrap();

	// prepare the dialog
	let modal = modal_stack.modal(|| InputMultiDialog::new().unwrap());
	let single_result = SingleResult::default();

	// set the title
	let title = aggregate_name(x_input, y_input).into();
	modal.dialog().set_dialog_title(title);

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(());
		CloseRequestResponse::KeepWindowShown
	});

	// set up the "ok" button
	let signaller = single_result.signaller();
	modal.dialog().on_ok_clicked(move || {
		signaller.signal(());
	});

	// dummy entries for now
	let dummy_entry = InputDialogEntry {
		name: "DUMMY".into(),
		text: "DUMMY".into(),
		primary_command: "".into(),
	};
	modal.dialog().set_left_entry(dummy_entry.clone());
	modal.dialog().set_right_entry(dummy_entry.clone());
	modal.dialog().set_up_entry(dummy_entry.clone());
	modal.dialog().set_down_entry(dummy_entry.clone());

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await
}

fn aggregate_name<'a>(x_input: Option<&'a Input>, y_input: Option<&'a Input>) -> &'a str {
	x_input
		.and_then(|input| {
			input
				.name
				.strip_suffix(['X', 'Y', 'Z'])
				.map(|name| name.trim_end_matches(char::is_whitespace))
		})
		.or_else(|| {
			y_input.and_then(|input| {
				input
					.name
					.strip_suffix(['X', 'Y', 'Z'])
					.map(|name| name.trim_end_matches(char::is_whitespace))
			})
		})
		.unwrap_or("<<UNKNOWN>>")
}
