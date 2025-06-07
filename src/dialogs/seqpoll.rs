use std::cell::Cell;

use serde::Deserialize;
use serde::Serialize;
use slint::CloseRequestResponse;

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::dialogs::SingleResult;
use crate::guiutils::modal::ModalStack;
use crate::runtime::command::MameCommand;
use crate::runtime::command::SeqType;
use crate::status::Input;
use crate::status::Status;
use crate::ui::SeqPollDialog;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SeqPollDialogType {
	Specify,
	Add,
}

#[allow(clippy::too_many_arguments)]
pub async fn dialog_seq_poll(
	modal_stack: ModalStack,
	port_tag: impl AsRef<str>,
	mask: u32,
	seq_type: SeqType,
	poll_type: SeqPollDialogType,
	inputs: impl AsRef<[Input]>,
	status_changed_channel: Channel<Status>,
	invoke_command: impl Fn(AppCommand) + Clone + 'static,
) {
	// prepare the dialog
	let modal = modal_stack.modal(|| SeqPollDialog::new().unwrap());
	let single_result = SingleResult::default();

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(());
		CloseRequestResponse::KeepWindowShown
	});

	// set the target name
	let input = inputs
		.as_ref()
		.iter()
		.find(|x| x.port_tag.as_ref() == port_tag.as_ref() && x.mask == mask)
		.unwrap();
	let target_name = input.name.as_str();
	let seq_tokens = match seq_type {
		SeqType::Standard => &input.seq_standard_tokens,
		SeqType::Decrement => &input.seq_decrement_tokens,
		SeqType::Increment => &input.seq_increment_tokens,
	};
	let (dialog_title, dialog_caption, start_seq) = match poll_type {
		SeqPollDialogType::Specify => (
			format!("Specify {target_name}"),
			format!("Press key or button to specify {target_name}"),
			"",
		),
		SeqPollDialogType::Add => (
			format!("Add To {target_name}"),
			format!("Press key or button to add to {target_name}"),
			seq_tokens.as_deref().unwrap_or_default(),
		),
	};
	modal.dialog().set_dialog_title(dialog_title.into());
	modal.dialog().set_dialog_caption(dialog_caption.into());

	// subscribe to status changes
	let polling_input_seq = Cell::new(false);
	let signaller = single_result.signaller();
	let _subscription = status_changed_channel.subscribe(move |status| {
		let running = status.running.as_ref();
		if running.is_none_or(|running| polling_input_seq.get() && !running.polling_input_seq) {
			signaller.signal(());
		} else if running.is_some_and(|running| running.polling_input_seq) {
			polling_input_seq.set(true);
		}
	});

	// invoke the command to start polling
	let command = MameCommand::seq_poll_start(port_tag, mask, seq_type, start_seq);
	invoke_command(command.into());

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await;

	// invoke the command to stop polling
	let command = MameCommand::seq_poll_stop();
	invoke_command(command.into());
}
