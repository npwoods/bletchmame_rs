use std::any::Any;
use std::borrow::Cow;
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
use slint::ToSharedString;
use slint::Weak;
use smol_str::SmolStr;
use strum::EnumProperty;
use strum::VariantArray;
use tokio::sync::mpsc;
use tracing::info;
use tracing::trace;
use tracing::trace_span;

use crate::action::Action;
use crate::channel::Channel;
use crate::dialogs::SenderExt;
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
use crate::status::InputDevice;
use crate::status::InputDeviceClass;
use crate::status::InputDeviceClassName;
use crate::status::InputDeviceToken;
use crate::status::Status;
use crate::ui::InputDialog;
use crate::ui::InputDialogCategory;
use crate::ui::InputDialogEntry;
use crate::ui::SimpleMenuEntry;
use crate::util::IteratorExt;

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
	pub categories: Box<[InputCategory]>,
	pub codes: HashMap<Box<str>, Box<str>>,
}

#[derive(Debug)]
struct InputCategory {
	pub info: Option<InputCategoryInfo>,
	pub cluster_indices: Box<[usize]>,
}

#[derive(Debug)]
struct InputCategoryInfo {
	pub player: u8,
	pub options: Vec<(SmolStr, Option<Action>)>,
	pub current_option_index: usize,
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

/// A model of the [`InputDialogEntry`]s that belong to a single category (player)
struct InputDialogCategoryModel {
	entries: Vec<InputDialogEntry>,
	notify: ModelNotify,
}

impl InputDialogCategoryModel {
	pub fn new(mut entries: Vec<InputDialogEntry>) -> Self {
		entries.shrink_to_fit();
		Self {
			entries,
			notify: ModelNotify::default(),
		}
	}
}

impl Model for InputDialogCategoryModel {
	type Data = InputDialogEntry;

	fn row_count(&self) -> usize {
		self.entries.len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		self.entries.get(row).cloned()
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

pub async fn dialog_input(
	modal_stack: ModalStack,
	inputs: Arc<[Input]>,
	input_device_classes: Arc<[InputDeviceClass]>,
	class: InputClass,
	status_update_channel: Channel<Status>,
	invoke_action: impl Fn(Action) + Clone + 'static,
) {
	// prepare the dialog
	let modal = modal_stack.modal(|| InputDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set the title
	modal.dialog().set_dialog_title(class.title().into());

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(());
		CloseRequestResponse::KeepWindowShown
	});

	// set up the context menu command handler
	modal.dialog().on_menu_item_action(move |command_string| {
		if let Some(command) = Action::decode_from_slint(command_string) {
			invoke_action(command);
		}
	});

	// set up the model
	let dialog_weak = modal.dialog().as_weak();
	let model = InputDialogModel::new(class, dialog_weak);
	let model = Rc::new(model);
	let model = ModelRc::new(model);
	let model_clone = model.clone();
	modal.dialog().set_categories(model_clone);

	// set up the context button clicked handler
	let dialog_weak = modal.dialog().as_weak();
	let model_clone = model.clone();
	modal
		.dialog()
		.on_context_button_clicked(move |category_index, entry_index, point| {
			let dialog = dialog_weak.unwrap();
			let model = InputDialogModel::get_model(&model_clone);
			let category_index = category_index.try_into().unwrap();
			let entry_index = entry_index.try_into().unwrap();
			let (entries_1, entries_2) = model.context_menu(category_index, entry_index);
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
	modal.run(async { rx.recv().await.unwrap() }).await;
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
				state.categories =
					build_categories(&state.inputs, &state.input_device_classes, &state.clusters, self.class);
				state.codes = build_codes(&state.input_device_classes);

				let action = build_set_bulk_action(&state.inputs, &state.clusters, BulkSet::Default);
				let action = action.encode_for_slint();
				self.dialog_weak.unwrap().set_restore_defaults_action(action);

				info!(inputs_len=?state.inputs.len(), input_device_classes_len=?state.input_device_classes.len(), "InputDialogModel::update(): Changing state");
				dump_clusters_trace(state.inputs.as_ref(), state.clusters.as_ref());
			}
			changed
		};

		if changed {
			self.notify.reset();
		}
	}

	pub fn context_menu(
		&self,
		category_index: usize,
		entry_index: usize,
	) -> (ModelRc<SimpleMenuEntry>, ModelRc<SimpleMenuEntry>) {
		let state = self.state.borrow();
		let inputs = state.inputs.as_ref();

		let cluster_index = state.categories[category_index].cluster_indices[entry_index];

		match &state.clusters[cluster_index] {
			InputCluster::Single(index) => {
				let input = &inputs[*index];
				let specify_command = Some(Action::seq_specify_dialog(input, SeqType::Standard));
				let add_command = Some(Action::seq_add_dialog(input, SeqType::Standard));
				let clear_command = Some(Action::seq_clear(input, SeqType::Standard));
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
				let specify_command = Some(Action::input_xy_dialog(x_input, y_input));
				let clear_command = Some(Action::set_multi_seq(x_input, y_input, "", "", "", "", "", ""));
				build_context_menu(&quick_item_inputs, quick_items, specify_command, None, clear_command)
			}
		}
	}

	pub fn get_model(model: &impl Model) -> &'_ Self {
		model.as_any().downcast_ref::<Self>().unwrap()
	}
}

impl Model for InputDialogModel {
	type Data = InputDialogCategory;

	fn row_count(&self) -> usize {
		let state = self.state.borrow();
		state.categories.len()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		// get the basics about this category
		let state = self.state.borrow();
		let category = state.categories.get(row)?;

		// does this category have info or is it anonymous?
		let (name, option_names, option_actions, current_option_index) = if let Some(info) = &category.info {
			let name = format!("Player {}", info.player).into();
			let option_names = info
				.options
				.iter()
				.map(|(name, _)| name.to_shared_string())
				.collect_model_rc();
			let option_actions = info
				.options
				.iter()
				.map(|(_, action)| action.as_ref().map(Action::encode_for_slint).unwrap_or_default())
				.collect_model_rc();
			(name, option_names, option_actions, Some(info.current_option_index))
		} else {
			// this is an anonymous category, so we don't have any name nor options to show
			Default::default()
		};
		let current_option_index = current_option_index.map(|x| x.try_into().unwrap()).unwrap_or(-1);

		// build the entries for this category
		let entries = category
			.cluster_indices
			.iter()
			.map(|cluster_index| build_entry(&state.inputs, &state.clusters[*cluster_index], &state.codes))
			.collect::<Vec<_>>();
		let entries = InputDialogCategoryModel::new(entries);
		let entries = ModelRc::new(entries);

		// and return the result
		let result = InputDialogCategory {
			name,
			entries,
			option_names,
			option_actions,
			current_option_index,
		};
		Some(result)
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}

fn build_entry(inputs: &[Input], cluster: &InputCluster, codes: &HashMap<Box<str>, Box<str>>) -> InputDialogEntry {
	let name = input_cluster_name(inputs, cluster).into();
	let input_seqs = input_cluster_input_seqs(inputs, cluster);
	let text = build_code_text(input_seqs, codes).to_shared_string();

	let primary_action = match cluster {
		InputCluster::Single(input_index) => {
			let input = &inputs[*input_index];
			Some(Action::seq_specify_dialog(input, SeqType::Standard))
		}
		InputCluster::Xy {
			x_input_index,
			y_input_index,
			..
		} => {
			let x_input = x_input_index.map(|idx| &inputs[idx]);
			let y_input = y_input_index.map(|idx| &inputs[idx]);
			let command = Action::input_xy_dialog(x_input, y_input);
			Some(command)
		}
	};
	let primary_action = primary_action
		.as_ref()
		.map(Action::encode_for_slint)
		.unwrap_or_default();

	InputDialogEntry {
		name,
		text,
		primary_action,
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

fn build_categories(
	inputs: &[Input],
	input_device_classes: &[InputDeviceClass],
	clusters: &[InputCluster],
	class: InputClass,
) -> Box<[InputCategory]> {
	clusters
		.iter()
		.enumerate()
		.map(|(index, cluster)| (cluster_player(inputs, class, cluster), index))
		.into_group_map()
		.into_iter()
		.sorted_by_key(|(player, _)| *player)
		.map(|(player, cluster_indices)| {
			let info = player.map(|player| {
				build_player_category_info(inputs, input_device_classes, clusters, &cluster_indices, player)
			});
			let cluster_indices = cluster_indices.into();
			InputCategory { info, cluster_indices }
		})
		.collect()
}

fn cluster_player(inputs: &[Input], class: InputClass, cluster: &InputCluster) -> Option<u8> {
	// we only use player categories for controllers; for other input classes the category is anonymous
	let use_player_categories = class == InputClass::Controller;

	use_player_categories
		.then(|| match cluster {
			InputCluster::Single(input_index) => Some(*input_index),
			InputCluster::Xy {
				x_input_index,
				y_input_index,
				..
			} => x_input_index.or(*y_input_index),
		})
		.flatten()
		.map(|input_index| inputs[input_index].player)
}

fn build_player_category_info(
	inputs: &[Input],
	input_device_classes: &[InputDeviceClass],
	clusters: &[InputCluster],
	cluster_indices: &[usize],
	player: u8,
) -> InputCategoryInfo {
	// it is simpler if we work in terms of "unclustered" inputs
	let unclustered_inputs = cluster_indices
		.iter()
		.map(|index| &clusters[*index])
		.flat_map(|cluster| match cluster {
			InputCluster::Single(index) => {
				/*
				let default_tokens = inputs[*index].seq_default_tokens(SeqType::Standard).unwrap_or_default();
				let token = device_item_token_from_default_tokens(default_tokens);
				 */
				vec![Some((*index, UnclusteredInputType::Standard))]
			}
			InputCluster::Xy {
				x_input_index,
				y_input_index,
				..
			} => vec![
				x_input_index.map(|index| (index, UnclusteredInputType::XAxis)),
				y_input_index.map(|index| (index, UnclusteredInputType::YAxis)),
			],
		})
		.flatten()
		.map(|(input_index, unclustered_input_type)| (&inputs[input_index], unclustered_input_type));

	// identify the input "sources" - this can be an input device (e.g. - gamepad, mouse), or it could be "None"
	let supported_device_classes = [InputDeviceClassName::Joystick, InputDeviceClassName::Mouse];
	let sources = [
		InputSource::None,
		InputSource::Default,
		InputSource::ArrowKeys,
		InputSource::NumberPad,
	]
	.into_iter()
	.chain(
		input_device_classes
			.iter_devices()
			.filter(|(device_class, _)| supported_device_classes.contains(&device_class.name))
			.map(|(device_class, device)| InputSource::Device(device_class, device)),
	);

	// now lets actually figure out the actual options
	let options = sources
		.map(|source| {
			// get the title of this device
			let title = SmolStr::from(source.title());

			// figure out all of the input seqs that would be specified if we selected
			// this input device - also figure out if it matches the current state
			let seqs = unclustered_inputs
				.clone()
				.flat_map(|(input, unclustered_input_type)| {
					// permute across the various seq types
					let seq_codes = if input.is_analog {
						vec![SeqType::Standard, SeqType::Increment, SeqType::Decrement]
					} else {
						vec![SeqType::Standard]
					};
					seq_codes
						.into_iter()
						.map(move |seq_type| (input, unclustered_input_type, seq_type))
				})
				.map(|(input, unclustered_input_type, seq_type)| {
					// find the code with the pertinent token on the device, and does it match what is
					// currently on the input?
					let code = source.find_code(input, unclustered_input_type, seq_type);
					let is_match = input.seq_tokens(seq_type) == code;
					(input.port_tag.as_str(), input.mask, seq_type, code, is_match)
				})
				.collect::<Vec<_>>();

			// its only a match if everything matches up
			let is_match = seqs.iter().all(|(_, _, _, _, is_match)| *is_match);

			// build the action; but we need to shuffle around the seqs first
			let seqs = seqs
				.into_iter()
				.map(|(tag, mask, seq_type, code, _)| (tag, mask, seq_type, code))
				.collect::<Vec<_>>();
			let action = Action::from(MameCommand::seq_set(&seqs));

			// finally return the info
			(title, action, is_match)
		})
		.collect::<Vec<_>>();

	// find the option that has is match on
	let current_option_index = options.iter().position(|(_, _, is_match)| *is_match);

	// reshuffle the options, to add the "(mixed)" item if necessary and to get rid of `is_match`
	let options = current_option_index
		.is_none()
		.then_some(("(mixed)".into(), None))
		.into_iter()
		.chain(options.into_iter().map(|(title, action, _)| (title, Some(action))))
		.collect::<Vec<_>>();

	// now we can get the real index
	let current_option_index = current_option_index.unwrap_or(0);

	// InputCategoryInfo::player is one counted
	let player = player + 1;

	// and we're done!
	InputCategoryInfo {
		player,
		options,
		current_option_index,
	}
}

#[derive(Debug, Clone, Copy)]
enum UnclusteredInputType {
	Standard,
	XAxis,
	YAxis,
}

#[derive(Debug, EnumProperty)]
enum InputSource<'a> {
	#[strum(props(Title = "None"))]
	None,
	#[strum(props(Title = "Default"))]
	Default,
	#[strum(props(Title = "Arrow Keys"))]
	ArrowKeys,
	#[strum(props(Title = "Number Pad"))]
	NumberPad,
	Device(&'a InputDeviceClass, &'a InputDevice),
}

impl<'a> InputSource<'a> {
	pub fn title(&self) -> Cow<'a, str> {
		match self {
			InputSource::Device(device_class, device) => {
				if let Some(prefix) = device_class.prefix() {
					format!("{} #{} ({})", prefix, device.devindex + 1, device.name).into()
				} else {
					device.name.as_str().into()
				}
			}
			_ => self
				.get_str("Title")
				.expect("missing title property on InputSource")
				.into(),
		}
	}

	pub fn find_code(
		&self,
		input: &'a Input,
		unclustered_input_type: UnclusteredInputType,
		seq_type: SeqType,
	) -> &'a str {
		let default_tokens = input.seq_default_tokens(seq_type);
		let token = InputSourceToken::new(default_tokens, unclustered_input_type, seq_type);

		match (self, token) {
			(InputSource::Default, _) => default_tokens,
			(InputSource::ArrowKeys, InputSourceToken::Left) => "KEYCODE_LEFT",
			(InputSource::ArrowKeys, InputSourceToken::Right) => "KEYCODE_RIGHT",
			(InputSource::ArrowKeys, InputSourceToken::Up) => "KEYCODE_UP",
			(InputSource::ArrowKeys, InputSourceToken::Down) => "KEYCODE_DOWN",
			(InputSource::NumberPad, InputSourceToken::Left) => "KEYCODE_4PAD",
			(InputSource::NumberPad, InputSourceToken::Right) => "KEYCODE_6PAD",
			(InputSource::NumberPad, InputSourceToken::Up) => "KEYCODE_8PAD",
			(InputSource::NumberPad, InputSourceToken::Down) => "KEYCODE_2PAD",
			(InputSource::NumberPad, InputSourceToken::Button(1)) => "KEYCODE_0PAD",
			(InputSource::NumberPad, InputSourceToken::Button(2)) => "KEYCODE_DELPAD",
			(InputSource::NumberPad, InputSourceToken::Button(3)) => "KEYCODE_ENTERPAD",
			(InputSource::NumberPad, InputSourceToken::Button(4)) => "KEYCODE_PLUSPAD",
			(InputSource::NumberPad, InputSourceToken::Button(5)) => "KEYCODE_MINUSPAD",
			(InputSource::Device(_, device), token) => {
				let token = token.as_input_device_token();
				device
					.items
					.iter()
					.find(|x| Some(&x.token) == token.as_ref())
					.map(|input_device_item| input_device_item.code.as_str())
					.unwrap_or_default()
			}
			(_, _) => "",
		}
	}
}

#[derive(Debug)]
enum InputSourceToken {
	Unknown,
	Left,
	Right,
	Up,
	Down,
	Button(u8),
	XAxis,
	YAxis,
	ZAxis,
	RZAxis,
}

impl InputSourceToken {
	pub fn new(default_tokens: &str, unclustered_input_type: UnclusteredInputType, seq_type: SeqType) -> Self {
		fn parse_single(token: &str) -> Option<InputSourceToken> {
			match token {
				"KEYCODE_LEFT" => Some(InputSourceToken::Left),
				"KEYCODE_RIGHT" => Some(InputSourceToken::Right),
				"KEYCODE_UP" => Some(InputSourceToken::Up),
				"KEYCODE_DOWN" => Some(InputSourceToken::Down),
				_ => {
					let rest = token
						.strip_prefix("JOYCODE_")
						.or_else(|| token.strip_prefix("MOUSECODE_"))?;
					let (number, code) = rest.split_once('_')?;
					let _ = number.parse::<u8>().ok()?;

					if let Some(rest) = code.strip_prefix("BUTTON") {
						Some(InputSourceToken::Button(rest.parse().ok()?))
					} else {
						match rest {
							"XAXIS" => Some(InputSourceToken::XAxis),
							"YAXIS" => Some(InputSourceToken::YAxis),
							"ZAXIS" => Some(InputSourceToken::ZAxis),
							"RZAXIS" => Some(InputSourceToken::RZAxis),
							_ => None,
						}
					}
				}
			}
		}

		match (unclustered_input_type, seq_type) {
			(UnclusteredInputType::XAxis, SeqType::Standard) => Self::XAxis,
			(UnclusteredInputType::XAxis, SeqType::Decrement) => Self::Left,
			(UnclusteredInputType::XAxis, SeqType::Increment) => Self::Right,
			(UnclusteredInputType::YAxis, SeqType::Standard) => Self::YAxis,
			(UnclusteredInputType::YAxis, SeqType::Decrement) => Self::Up,
			(UnclusteredInputType::YAxis, SeqType::Increment) => Self::Down,
			(UnclusteredInputType::Standard, SeqType::Standard) => default_tokens
				.split(" OR ")
				.filter_map(parse_single)
				.next()
				.unwrap_or(Self::Unknown),
			_ => Self::Unknown,
		}
	}

	fn as_input_device_token(&self) -> Option<InputDeviceToken> {
		match self {
			InputSourceToken::Button(num) => Some(InputDeviceToken::Button(*num)),
			InputSourceToken::XAxis => Some(InputDeviceToken::XAxis),
			InputSourceToken::YAxis => Some(InputDeviceToken::YAxis),
			InputSourceToken::ZAxis => Some(InputDeviceToken::ZAxis),
			InputSourceToken::RZAxis => Some(InputDeviceToken::RZAxis),
			_ => None,
		}
	}
}

#[derive(Debug)]
enum BulkSet {
	Default,
}

fn build_set_bulk_action<'a>(
	inputs: &[Input],
	clusters: impl IntoIterator<Item = &'a InputCluster>,
	bulk_set: BulkSet,
) -> Action {
	let bulk_set_string = match bulk_set {
		BulkSet::Default => "*",
	};

	let seqs = clusters
		.into_iter()
		.flat_map(|cluster| input_cluster_input_seqs(inputs, cluster))
		.map(|(input, _, seq_type)| (&input.port_tag, input.mask, seq_type, bulk_set_string))
		.collect::<Vec<_>>();

	MameCommand::seq_set(&seqs).into()
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
