use std::borrow::Cow;
use std::cell::Cell;

use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;
use slint::CloseRequestResponse;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use tokio::sync::mpsc;

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::runtime::command::MameCommand;
use crate::runtime::command::SeqType;
use crate::status::Input;
use crate::status::InputDeviceClass;
use crate::status::InputDeviceClassName;
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
	input_device_classes: impl AsRef<[InputDeviceClass]>,
	status_changed_channel: Channel<Status>,
	invoke_command: impl Fn(AppCommand) + Clone + 'static,
) {
	// prepare the dialog
	let modal = modal_stack.modal(|| SeqPollDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// set the target name
	let input = inputs
		.as_ref()
		.iter()
		.find(|x| x.port_tag.as_ref() == port_tag.as_ref() && x.mask == mask)
		.unwrap();
	let target_name = input.name.as_str();
	let target_name_suffix = seq_type.suffix();
	let seq_tokens = match seq_type {
		SeqType::Standard => &input.seq_standard_tokens,
		SeqType::Decrement => &input.seq_decrement_tokens,
		SeqType::Increment => &input.seq_increment_tokens,
	};
	let (dialog_title, dialog_caption, start_seq) = match poll_type {
		SeqPollDialogType::Specify => (
			format!("Specify {target_name}{target_name_suffix}"),
			format!("Press key or button to specify {target_name}{target_name_suffix}"),
			"",
		),
		SeqPollDialogType::Add => (
			format!("Add To {target_name}{target_name_suffix}"),
			format!("Press key or button to add to {target_name}{target_name_suffix}"),
			seq_tokens.as_deref().unwrap_or_default(),
		),
	};
	modal.dialog().set_dialog_title(dialog_title.into());
	modal.dialog().set_dialog_caption(dialog_caption.into());

	// identify and build mouse input items
	let (mouse_input_titles, mouse_input_commands): (Vec<_>, Vec<_>) = input_device_classes
		.as_ref()
		.iter()
		.filter(|device_class| device_class.name == InputDeviceClassName::Mouse)
		.flat_map(|device_class| &device_class.devices)
		.flat_map(|device| &device.items)
		.filter(|item| !item.token.is_axis())
		.map(|item| {
			let codes = if start_seq.is_empty() {
				Cow::Borrowed(item.code.as_str())
			} else {
				format!("{} or {}", start_seq, item.code.as_str()).into()
			};
			let seqs = [(port_tag.as_ref(), mask, seq_type, codes)];
			let command = MameCommand::seq_set(&seqs);
			(item.name.as_str(), command)
		})
		.sorted_by_key(|(name, _)| (*name))
		.map(|(name, command)| (SharedString::from(name), command))
		.unzip();

	// set up the mouse input items menu...
	let mouse_input_titles = VecModel::from(mouse_input_titles);
	let mouse_input_titles = ModelRc::new(mouse_input_titles);
	modal.dialog().set_mouse_input_titles(mouse_input_titles);

	// ...and also the corresponding callback
	let tx_clone = tx.clone();
	modal.dialog().on_mouse_input_selected(move |index| {
		let index = usize::try_from(index).unwrap();
		tx_clone.signal(Some(index));
	});

	// subscribe to status changes
	let polling_input_seq = Cell::new(false);
	let tx_clone = tx.clone();
	let _subscription = status_changed_channel.subscribe(move |status| {
		let running = status.running.as_ref();
		if running.is_none_or(|running| polling_input_seq.get() && !running.polling_input_seq) {
			tx_clone.signal(None);
		} else if running.is_some_and(|running| running.polling_input_seq) {
			polling_input_seq.set(true);
		}
	});

	// invoke the command to start polling
	let command = MameCommand::seq_poll_start(port_tag, mask, seq_type, start_seq);
	invoke_command(command.into());

	// present the modal dialog
	let mouse_selection = modal.run(async { rx.recv().await.unwrap() }).await;

	// invoke the command to stop polling
	let command = MameCommand::seq_poll_stop();
	invoke_command(command.into());

	// and if we have a mouse selection, specify it
	if let Some(mouse_selection) = mouse_selection {
		let command = mouse_input_commands.into_iter().nth(mouse_selection).unwrap();
		invoke_command(command.into());
	}
}
