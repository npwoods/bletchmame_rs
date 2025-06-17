use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use itertools::Either;
use itertools::Itertools;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::Weak;
use strum::VariantArray;
use tracing::info;
use tracing::trace;
use tracing::trace_span;

use crate::appcommand::AppCommand;
use crate::channel::Channel;
use crate::dialogs::SingleResult;
use crate::dialogs::input::InputAxis;
use crate::dialogs::input::InputDeviceClassExt;
use crate::dialogs::input::InputDeviceClassSliceExt;
use crate::dialogs::input::InputSeqExt;
use crate::dialogs::input::build_code_text;
use crate::dialogs::input::build_codes;
use crate::dialogs::input::build_context_menu;
use crate::guiutils::modal::ModalStack;
use crate::runtime::command::MameCommand;
use crate::runtime::command::SeqType;
use crate::status::Input;
use crate::status::InputClass;
use crate::status::InputDeviceClass;
use crate::status::InputDeviceToken;
use crate::status::Status;
use crate::ui::InputContextMenuEntry;
use crate::ui::InputDialog;
use crate::ui::InputDialogEntry;

struct InputDialogModel {
	dialog_weak: Weak<InputDialog>,
	state: RefCell<InputDialogState>,
	class: InputClass,
	notify: ModelNotify,
}

#[derive(Debug, Default)]
struct InputDialogState {
	pub inputs: Arc<[Input]>,
	pub input_device_classes: Arc<[InputDeviceClass]>,
	pub clusters: Box<[InputCluster]>,
	pub codes: HashMap<Box<str>, Box<str>>,
}

#[derive(Debug)]
enum InputCluster {
	Single(usize),
	Xy {
		x_input_index: Option<usize>,
		y_input_index: Option<usize>,
		aggregate_name: Option<String>,
	},
}

pub async fn dialog_input(
	modal_stack: ModalStack,
	inputs: Arc<[Input]>,
	input_device_classes: Arc<[InputDeviceClass]>,
	class: InputClass,
	status_update_channel: Channel<Status>,
	invoke_command: impl Fn(AppCommand) + Clone + 'static,
) {
	// prepare the dialog
	let modal = modal_stack.modal(|| InputDialog::new().unwrap());
	let single_result = SingleResult::default();

	// set the title
	modal.dialog().set_dialog_title(class.title().into());

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

	// set up the context menu command handler
	modal.dialog().on_menu_item_command(move |command_string| {
		if let Some(command) = AppCommand::decode_from_slint(command_string) {
			invoke_command(command);
		}
	});

	// set up the model
	let dialog_weak = modal.dialog().as_weak();
	let model = InputDialogModel::new(class, dialog_weak);
	let model = Rc::new(model);
	let model = ModelRc::new(model);
	let model_clone = model.clone();
	modal.dialog().set_entries(model_clone);

	// set up the context button clicked handler
	let dialog_weak = modal.dialog().as_weak();
	let model_clone = model.clone();
	modal.dialog().on_context_button_clicked(move |index, point| {
		let dialog = dialog_weak.unwrap();
		let model = InputDialogModel::get_model(&model_clone);
		let index = index.try_into().unwrap();
		let (entries_1, entries_2) = model.context_menu(index);
		dialog.invoke_show_context_menu(entries_1, entries_2, point);
	});

	// subscribe to status changes
	let model_clone = model.clone();
	let _subscription = status_update_channel.subscribe(move |status| {
		// update the model
		let model = InputDialogModel::get_model(&model_clone);
		let running = status.running.as_ref();
		let inputs = running.map(|r| &r.inputs).cloned().unwrap_or_default();
		let input_device_classes = running.map(|r| &r.input_device_classes).cloned().unwrap_or_default();
		model.update(inputs, input_device_classes);
	});

	// update the model
	InputDialogModel::get_model(&model).update(inputs, input_device_classes);

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await
}

impl InputDialogModel {
	pub fn new(class: InputClass, dialog_weak: Weak<InputDialog>) -> Self {
		let state = InputDialogState::default();
		let state = RefCell::new(state);
		let notify = ModelNotify::default();
		Self {
			dialog_weak,
			state,
			class,
			notify,
		}
	}

	pub fn update(&self, inputs: Arc<[Input]>, input_device_classes: Arc<[InputDeviceClass]>) {
		let changed = {
			let mut state = self.state.borrow_mut();
			let changed = state.inputs != inputs || state.input_device_classes != input_device_classes;
			if changed {
				state.inputs = inputs;
				state.input_device_classes = input_device_classes;
				state.clusters = build_clusters(&state.inputs, self.class);
				state.codes = build_codes(&state.input_device_classes);

				let command = build_restore_defaults_command(&state.inputs, &state.clusters);
				let command = command.as_ref().map(AppCommand::encode_for_slint).unwrap_or_default();
				self.dialog_weak.unwrap().set_restore_defaults_command(command);

				info!(inputs_len=?state.inputs.len(), input_device_classes_len=?state.input_device_classes.len(), "InputDialogModel::update(): Changing state");
				dump_clusters_trace(state.inputs.as_ref(), state.clusters.as_ref());
			}
			changed
		};

		if changed {
			self.notify.reset();
		}
	}

	pub fn context_menu(&self, index: usize) -> (ModelRc<InputContextMenuEntry>, ModelRc<InputContextMenuEntry>) {
		let state = self.state.borrow();
		let inputs = state.inputs.as_ref();

		match &state.clusters[index] {
			InputCluster::Single(index) => {
				let input = &inputs[*index];
				let specify_command = Some(AppCommand::seq_specify_dialog(input, SeqType::Standard));
				let add_command = Some(AppCommand::seq_add_dialog(input, SeqType::Standard));
				let clear_command = Some(AppCommand::seq_clear(input, SeqType::Standard));
				build_context_menu(&[], [], specify_command, add_command, clear_command)
			}
			InputCluster::Xy {
				x_input_index,
				y_input_index,
				..
			} => {
				let x_input = x_input_index.map(|index| &inputs[index]);
				let y_input = y_input_index.map(|index| &inputs[index]);

				// prepare builtin quick items
				let arrow_keys_seqs = vec![
					(0, SeqType::Standard, ""),
					(0, SeqType::Decrement, "KEYCODE_LEFT"),
					(0, SeqType::Increment, "KEYCODE_RIGHT"),
					(1, SeqType::Standard, ""),
					(1, SeqType::Decrement, "KEYCODE_UP"),
					(1, SeqType::Increment, "KEYCODE_DOWN"),
				];
				let numpad_seqs = vec![
					(0, SeqType::Standard, ""),
					(0, SeqType::Decrement, "KEYCODE_4PAD"),
					(0, SeqType::Increment, "KEYCODE_6PAD"),
					(1, SeqType::Standard, ""),
					(1, SeqType::Decrement, "KEYCODE_2PAD"),
					(1, SeqType::Increment, "KEYCODE_8PAD"),
				];
				let builtin_entries = [
					("Arrow Keys".into(), arrow_keys_seqs),
					("Number Pad".into(), numpad_seqs),
				];

				// prepare input device-specific quick items
				let device_entries_iter = state.input_device_classes.iter_devices().map(|(device_class, device)| {
					let title = if let Some(prefix) = device_class.prefix() {
						format!("{} #{} ({})", prefix, device.devindex + 1, device.name).into()
					} else {
						device.name.as_str().into()
					};
					let codes = device
						.items
						.iter()
						.flat_map(|item| {
							let input_index = match item.token {
								InputDeviceToken::XAxis => Some(0),
								InputDeviceToken::YAxis => Some(1),
								_ => None,
							};
							SeqType::VARIANTS
								.iter()
								.filter_map(move |seq_type| input_index.map(|input_index| (input_index, seq_type)))
								.map(|(input_index, seq_type)| {
									let code = if *seq_type == SeqType::Standard { &item.code } else { "" };
									(input_index, *seq_type, code)
								})
						})
						.collect::<Vec<_>>();
					(title, codes)
				});

				// and combine them
				let quick_items = builtin_entries.into_iter().chain(device_entries_iter);

				// finally build the context menu
				let quick_item_inputs = [x_input, y_input];
				let specify_command = Some(AppCommand::input_xy_dialog(x_input, y_input));
				let clear_command = Some(AppCommand::set_multi_seq(x_input, y_input, "", "", "", "", "", ""));
				build_context_menu(&quick_item_inputs, quick_items, specify_command, None, clear_command)
			}
		}
	}

	pub fn get_model(model: &impl Model) -> &'_ Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for InputDialogModel {
	type Data = InputDialogEntry;

	fn row_count(&self) -> usize {
		let state = self.state.borrow();
		state.clusters.len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		let state = self.state.borrow();
		let cluster = state.clusters.get(row)?;
		let name = input_cluster_name(&state.inputs, cluster).into();
		let input_seqs = input_cluster_input_seqs(&state.inputs, cluster);
		let text = build_code_text(input_seqs, &state.codes).as_ref().into();

		let primary_command = match cluster {
			InputCluster::Single(input_index) => {
				let input = &state.inputs[*input_index];
				Some(AppCommand::seq_specify_dialog(input, SeqType::Standard))
			}
			InputCluster::Xy {
				x_input_index,
				y_input_index,
				..
			} => {
				let x_input = x_input_index.map(|idx| &state.inputs[idx]);
				let y_input = y_input_index.map(|idx| &state.inputs[idx]);
				let command = AppCommand::input_xy_dialog(x_input, y_input);
				Some(command)
			}
		};
		let primary_command = primary_command
			.as_ref()
			.map(AppCommand::encode_for_slint)
			.unwrap_or_default();

		Some(InputDialogEntry {
			name,
			text,
			primary_command,
		})
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

fn build_clusters(inputs: &[Input], class: InputClass) -> Box<[InputCluster]> {
	inputs
		.iter()
		.enumerate()
		.filter(move |(_, input)| input.class == Some(class))
		.sorted_by_key(|(_, input)| {
			(
				input.group,
				input.input_type,
				input.first_keyboard_code.unwrap_or_default(),
				&input.name,
			)
		})
		.coalesce(|a, b| {
			// because of how the LUA "fields" property works, there may be dupes in this data, and
			// this logic removes the dupes
			if a.1.port_tag == b.1.port_tag && a.1.mask == b.1.mask {
				Ok(a)
			} else {
				Err((a, b))
			}
		})
		.map(|(index, input)| input_cluster_from_input(index, input))
		.coalesce(|a, b| coalesce_input_clusters(&a, &b).ok_or((a, b)))
		.collect()
}

fn build_restore_defaults_command(inputs: &[Input], clusters: &[InputCluster]) -> Option<AppCommand> {
	let seqs = clusters
		.iter()
		.flat_map(|cluster| input_cluster_input_seqs(inputs, cluster))
		.map(|(input, _, seq_type)| (input.port_tag.as_ref(), input.mask, seq_type, "*"))
		.collect::<Vec<_>>();

	let command = MameCommand::seq_set(&seqs);
	Some(command.into())
}

fn input_cluster_from_input(index: usize, input: &Input) -> InputCluster {
	if input.is_analog {
		let name = input
			.name
			.trim_end_matches(|ch: char| ch.is_ascii_digit() || ch.is_whitespace());
		let (x_input_index, y_input_index) = if name.ends_with('Y') {
			(None, Some(index))
		} else {
			(Some(index), None)
		};

		let aggregate_name = name
			.strip_suffix(['X', 'Y', 'Z'])
			.map(|name| name.trim_end_matches(char::is_whitespace).to_string());

		InputCluster::Xy {
			x_input_index,
			y_input_index,
			aggregate_name,
		}
	} else {
		InputCluster::Single(index)
	}
}

fn input_cluster_name<'a>(inputs: &'a [Input], cluster: &'a InputCluster) -> &'a str {
	match cluster {
		InputCluster::Single(input_index) => &inputs[*input_index].name,
		InputCluster::Xy {
			x_input_index,
			y_input_index,
			aggregate_name,
		} => aggregate_name
			.as_deref()
			.or_else(|| x_input_index.map(|idx| inputs[idx].name.as_str()))
			.or_else(|| y_input_index.map(|idx| inputs[idx].name.as_str()))
			.unwrap(),
	}
}

fn input_cluster_input_seqs<'a>(
	inputs: &'a [Input],
	cluster: &InputCluster,
) -> impl Iterator<Item = (&'a Input, Option<InputAxis>, SeqType)> {
	match cluster {
		InputCluster::Single(input_index) => {
			let input = &inputs[*input_index];
			Either::Left([(input, None, SeqType::Standard)].into_iter())
		}
		InputCluster::Xy {
			x_input_index,
			y_input_index,
			..
		} => {
			let inputs = [
				x_input_index.map(|idx| (&inputs[idx], InputAxis::X)),
				y_input_index.map(|idx| (&inputs[idx], InputAxis::Y)),
			];
			let results_iter = inputs.into_iter().flatten().flat_map(|(input, axis)| {
				[SeqType::Standard, SeqType::Decrement, SeqType::Increment]
					.into_iter()
					.map(move |seq_type| (input, Some(axis), seq_type))
			});
			Either::Right(results_iter)
		}
	}
}

fn input_cluster_as_multi(input_cluster: &InputCluster) -> Option<(Option<usize>, Option<usize>, Option<&'_ str>)> {
	match input_cluster {
		InputCluster::Single(_) => None,
		InputCluster::Xy {
			x_input_index,
			y_input_index,
			aggregate_name,
		} => Some((*x_input_index, *y_input_index, aggregate_name.as_deref())),
	}
}

fn coalesce_input_clusters(a: &InputCluster, b: &InputCluster) -> Option<InputCluster> {
	let (a_x_input_index, a_y_input_index, a_aggregate_name) = input_cluster_as_multi(a)?;
	let (b_x_input_index, b_y_input_index, b_aggregate_name) = input_cluster_as_multi(b)?;
	if a_aggregate_name != b_aggregate_name {
		return None;
	}

	let (x_input_index, y_input_index) = match (a_x_input_index, a_y_input_index, b_x_input_index, b_y_input_index) {
		(Some(x_input_index), None, None, Some(y_input_index)) => Some((x_input_index, y_input_index)),
		(None, Some(x_input_index), Some(y_input_index), None) => Some((x_input_index, y_input_index)),
		_ => None,
	}?;
	let x_input_index = Some(x_input_index);
	let y_input_index = Some(y_input_index);
	let aggregate_name = a_aggregate_name.map(|x| x.to_string());

	let result = InputCluster::Xy {
		x_input_index,
		y_input_index,
		aggregate_name,
	};
	Some(result)
}

fn dump_clusters_trace(inputs: &[Input], clusters: &[InputCluster]) {
	let span = trace_span!("dump_clusters_trace");
	let _guard = span.enter();

	for (idx, cluster) in clusters.iter().enumerate() {
		match cluster {
			InputCluster::Single(input_index) => {
				if let Some(input) = inputs.get(*input_index) {
					trace!("Cluster[{idx}]: {input:?}");
				}
			}
			InputCluster::Xy {
				x_input_index,
				y_input_index,
				..
			} => {
				if let Some(input) = x_input_index.and_then(|i| inputs.get(i)) {
					trace!("Cluster[{idx}].X: {input:?}");
				}
				if let Some(input) = y_input_index.and_then(|i| inputs.get(i)) {
					trace!("Cluster[{idx}].Y: {input:?}");
				}
			}
		}
	}
}
