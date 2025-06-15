use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::LogicalPosition;
use slint::ModelRc;
use slint::VecModel;
use slint::Weak;
use strum::VariantArray;

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::dialogs::SingleResult;
use crate::dialogs::input::InputAxis;
use crate::dialogs::input::InputDeviceClassExt;
use crate::dialogs::input::InputDeviceClassSliceExt;
use crate::dialogs::input::InputSeqExt;
use crate::dialogs::input::build_code_text;
use crate::dialogs::input::build_codes;
use crate::dialogs::seqpoll::SeqPollDialogType;
use crate::guiutils::modal::ModalStack;
use crate::runtime::command::MameCommand;
use crate::runtime::command::SeqType;
use crate::status::Input;
use crate::status::InputDeviceClass;
use crate::status::Status;
use crate::ui::InputContextMenuEntry;
use crate::ui::InputDialogEntry;
use crate::ui::InputXyDialog;

struct Model {
	dialog_weak: Weak<InputXyDialog>,
	x_input: Option<(Arc<str>, u32)>,
	y_input: Option<(Arc<str>, u32)>,
	state: RefCell<State>,
}

#[derive(Debug, Default)]
struct State {
	inputs: Arc<[Input]>,
	input_device_classes: Arc<[InputDeviceClass]>,
}

pub async fn dialog_input_xy(
	modal_stack: ModalStack,
	x_input: Option<(Arc<str>, u32)>,
	y_input: Option<(Arc<str>, u32)>,
	inputs: Arc<[Input]>,
	input_device_classes: Arc<[InputDeviceClass]>,
	status_changed_channel: Channel<Status>,
	invoke_command: impl Fn(AppCommand) + Clone + 'static,
) {
	// prepare the dialog
	let modal = modal_stack.modal(|| InputXyDialog::new().unwrap());
	let single_result = SingleResult::default();

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

	// set up command handler
	modal.dialog().on_menu_item_command(move |command_string| {
		if let Some(command) = AppCommand::decode_from_slint(command_string) {
			invoke_command(command);
		}
	});

	// set up model
	let dialog_weak = modal.dialog().as_weak();
	let model = Model::new(dialog_weak, x_input, y_input);
	let model = Rc::new(model);
	model.update(inputs, input_device_classes);

	// set up context button menus
	modal.dialog().on_left_context_button_clicked({
		let model = model.clone();
		move |point| model.context_menu(point, InputAxis::X, SeqType::Decrement)
	});
	modal.dialog().on_right_context_button_clicked({
		let model = model.clone();
		move |point| model.context_menu(point, InputAxis::X, SeqType::Increment)
	});
	modal.dialog().on_up_context_button_clicked({
		let model = model.clone();
		move |point| model.context_menu(point, InputAxis::Y, SeqType::Decrement)
	});
	modal.dialog().on_down_context_button_clicked({
		let model = model.clone();
		move |point| model.context_menu(point, InputAxis::Y, SeqType::Increment)
	});

	// subscribe to update events
	let model_clone = model.clone();
	let _subscription = status_changed_channel.subscribe(move |status| {
		// update the model
		let running = status.running.as_ref();
		let inputs = running.map(|r| &r.inputs).cloned().unwrap_or_default();
		let input_device_classes = running.map(|r| &r.input_device_classes).cloned().unwrap_or_default();
		model_clone.update(inputs, input_device_classes);
	});

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await
}

impl Model {
	pub fn new(
		dialog_weak: Weak<InputXyDialog>,
		x_input: Option<(Arc<str>, u32)>,
		y_input: Option<(Arc<str>, u32)>,
	) -> Self {
		Self {
			dialog_weak,
			x_input,
			y_input,
			state: Default::default(),
		}
	}

	pub fn update(&self, inputs: Arc<[Input]>, input_device_classes: Arc<[InputDeviceClass]>) {
		let mut state = self.state.borrow_mut();
		if (inputs.as_ref() == state.inputs.as_ref())
			&& (input_device_classes.as_ref() == state.input_device_classes.as_ref())
		{
			return;
		}

		// update our core state
		state.inputs = inputs;
		state.input_device_classes = input_device_classes;

		// get the dialog
		let dialog = self.dialog_weak.unwrap();

		// look up the inputs
		let x_input = lookup_input(&state.inputs, &self.x_input);
		let y_input = lookup_input(&state.inputs, &self.y_input);

		// set the title
		let title = aggregate_name(x_input, y_input).into();
		dialog.set_dialog_title(title);

		// build the codes
		let codes = build_codes(&state.input_device_classes);

		// and specify the four entries
		let entry = build_input_dialog_entry(x_input, InputAxis::X, SeqType::Decrement, &codes);
		dialog.set_left_entry(entry);
		let entry = build_input_dialog_entry(x_input, InputAxis::X, SeqType::Increment, &codes);
		dialog.set_right_entry(entry);
		let entry = build_input_dialog_entry(y_input, InputAxis::Y, SeqType::Decrement, &codes);
		dialog.set_up_entry(entry);
		let entry = build_input_dialog_entry(y_input, InputAxis::Y, SeqType::Increment, &codes);
		dialog.set_down_entry(entry);

		// set up clear and restore defaults commands
		let clear_command = set_all_seqs_command(x_input, y_input, "");
		let restore_defaults_command = set_all_seqs_command(x_input, y_input, "*");
		dialog.set_clear_command(clear_command.encode_for_slint());
		dialog.set_restore_defaults_command(restore_defaults_command.encode_for_slint());
	}

	pub fn context_menu(&self, point: LogicalPosition, axis: InputAxis, seq_type: SeqType) {
		let (entries_1, entries_2) = {
			let state = self.state.borrow();

			// identify the input
			let input = match axis {
				InputAxis::X => &self.x_input,
				InputAxis::Y => &self.y_input,
			};
			let input = lookup_input(&state.inputs, input).unwrap();

			let entries_1 = state
				.input_device_classes
				.iter_device_items()
				.filter(|(_, _, item)| {
					item.token == "XAXIS" || item.token == "YAXIS" || item.token == "ZAXIS" || item.token == "RZAXIS"
				})
				.map(|(device_class, device, item)| {
					let title = if let Some(prefix) = device_class.prefix() {
						format!("{} #{} {} ({})", prefix, device.devindex + 1, item.name, device.name).into()
					} else {
						format!("{} ({})", item.name, device.name).into()
					};
					let seqs = [(&input.port_tag, input.mask, SeqType::Standard, &item.code)];
					let command = Some(MameCommand::seq_set(&seqs).into());
					let command = command.as_ref().map(AppCommand::encode_for_slint).unwrap_or_default();
					InputContextMenuEntry { title, command }
				})
				.collect::<Vec<_>>();

			let entries_2 = [
				("Specify...", AppCommand::seq_specify_dialog(input, seq_type)),
				("Add...", AppCommand::seq_add_dialog(input, seq_type)),
				("Clear", AppCommand::seq_clear(input, seq_type)),
			];
			let entries_2 = entries_2
				.into_iter()
				.map(|(title, command)| {
					let title = title.into();
					let command = command.encode_for_slint();
					InputContextMenuEntry { title, command }
				})
				.collect::<Vec<_>>();

			(entries_1, entries_2)
		};

		let entries_1 = VecModel::from(entries_1);
		let entries_1 = ModelRc::new(entries_1);

		let entries_2 = VecModel::from(entries_2);
		let entries_2 = ModelRc::new(entries_2);

		let dialog = self.dialog_weak.unwrap();
		dialog.invoke_show_context_menu(entries_1, entries_2, point);
	}
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

fn lookup_input<'a>(inputs: &'a [Input], this_input: &Option<(Arc<str>, u32)>) -> Option<&'a Input> {
	this_input.as_ref().and_then(|(port_tag, mask)| {
		let target = (port_tag.as_ref(), mask);
		inputs.iter().find(|x| target == (x.port_tag.as_ref(), mask))
	})
}

fn build_input_dialog_entry(
	input: Option<&Input>,
	axis: InputAxis,
	seq_type: SeqType,
	codes: &HashMap<Box<str>, impl AsRef<str>>,
) -> InputDialogEntry {
	if let Some(input) = input {
		let input_seqs = [(input, Some(axis), SeqType::Standard), (input, Some(axis), seq_type)];
		let suffix = seq_type.suffix();
		let name = format!("{}{}", &input.name, suffix).into();
		let text = build_code_text(input_seqs, codes).as_ref().into();
		let primary_command = AppCommand::SeqPollDialog {
			port_tag: input.port_tag.clone(),
			mask: input.mask,
			seq_type,
			poll_type: SeqPollDialogType::Specify,
		};
		let primary_command = primary_command.encode_for_slint();
		InputDialogEntry {
			name,
			text,
			primary_command,
		}
	} else {
		InputDialogEntry::default()
	}
}

fn set_all_seqs_command(x_input: Option<&Input>, y_input: Option<&Input>, tokens: &str) -> AppCommand {
	let seqs = [x_input, y_input]
		.into_iter()
		.flatten()
		.flat_map(|input| SeqType::VARIANTS.iter().map(move |seq_type| (input, *seq_type)))
		.map(|(input, seq_type)| (input.port_tag.as_ref(), input.mask, seq_type, tokens))
		.collect::<Vec<_>>();
	MameCommand::seq_set(&seqs).into()
}
