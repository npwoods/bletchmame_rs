use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

use itertools::Either;
use itertools::Itertools;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;
use strum::EnumString;

use crate::dialogs::SingleResult;
use crate::guiutils::modal::Modal;
use crate::status::Input;
use crate::status::InputClass;
use crate::status::InputDeviceClass;
use crate::status::InputDeviceClassName;
use crate::ui::InputContextMenuEntry;
use crate::ui::InputDialog;
use crate::ui::InputDialogEntry;

struct InputDialogModel {
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
	Multi {
		x_input_index: Option<usize>,
		y_input_index: Option<usize>,
		aggregate_name: Option<String>,
	},
}

#[derive(Debug, PartialEq)]
enum SeqToken<'a> {
	Named(&'a str, Option<SeqTokenModifier<'a>>),
	Or,
	Not,
	Default,
}

#[derive(Debug, PartialEq, EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
enum SeqTokenModifier<'a> {
	LeftSwitch,
	UpSwitch,
	RightSwitch,
	DownSwitch,
	Relative,
	Absolute,
	#[strum(disabled)]
	Unknown(&'a str),
}

pub async fn dialog_input(
	parent: Weak<impl ComponentHandle + 'static>,
	inputs: Arc<[Input]>,
	input_device_classes: Arc<[InputDeviceClass]>,
	class: InputClass,
) {
	// prepare the dialog
	let modal = Modal::new(&parent.unwrap(), || InputDialog::new().unwrap());
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

	// set up the model
	let model = InputDialogModel::new(class);
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

	// update the model
	InputDialogModel::get_model(&model).update(inputs, input_device_classes);

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await
}

impl InputDialogModel {
	pub fn new(class: InputClass) -> Self {
		let state = InputDialogState::default();
		let state = RefCell::new(state);
		let notify = ModelNotify::default();
		Self { state, class, notify }
	}

	pub fn update(&self, inputs: Arc<[Input]>, input_device_classes: Arc<[InputDeviceClass]>) {
		let mut state = self.state.borrow_mut();
		state.inputs = inputs;
		state.input_device_classes = input_device_classes;
		state.clusters = build_clusters(&state.inputs, self.class);
		state.codes = build_codes(&state.input_device_classes);
		drop(state);

		self.notify.reset();
	}

	pub fn context_menu(&self, index: usize) -> (ModelRc<InputContextMenuEntry>, ModelRc<InputContextMenuEntry>) {
		let (entries_1, entries_2) = {
			let state = self.state.borrow();
			let cluster = &state.clusters[index];

			match cluster {
				InputCluster::Single(_) => {
					let entries_1 = [].into();
					let entries_2 = ["Specify", "Add...", "Clear"]
						.iter()
						.copied()
						.map(|text| {
							let text = SharedString::from(text);
							InputContextMenuEntry { text }
						})
						.collect::<Vec<_>>();
					(entries_1, entries_2)
				}
				InputCluster::Multi {
					x_input_index,
					y_input_index,
					..
				} => {
					let entries_1 = (x_input_index.is_some() || y_input_index.is_some())
						.then(|| {
							let device_entry_iter = state
								.input_device_classes
								.iter()
								.flat_map(|device_class| &device_class.devices)
								.filter(|device| {
									let has_x = device.items.iter().any(|item| item.token == "XAXIS");
									let has_y = device.items.iter().any(|item| item.token == "YAXIS");
									(x_input_index.is_some() && has_x) || (y_input_index.is_some() && has_y)
								})
								.map(|device| device.name.as_str());

							["Arrow Keys", "Numeric Keypad"]
								.iter()
								.copied()
								.chain(device_entry_iter)
								.chain(["Multiple..."].iter().copied())
								.map(|text| {
									let text = SharedString::from(text);
									InputContextMenuEntry { text }
								})
								.collect::<Vec<_>>()
						})
						.unwrap_or_default();

					let entries_2 = ["Specify", "Clear"]
						.iter()
						.copied()
						.map(|text| {
							let text = SharedString::from(text);
							InputContextMenuEntry { text }
						})
						.collect::<Vec<_>>();
					(entries_1, entries_2)
				}
			}
		};

		let entries_1 = VecModel::from(entries_1);
		let entries_2 = VecModel::from(entries_2);
		let entries_1 = ModelRc::new(entries_1);
		let entries_2 = ModelRc::new(entries_2);
		(entries_1, entries_2)
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
		let text = input_cluster_code_text(&state.inputs, cluster, &state.codes).into();
		Some(InputDialogEntry { name, text })
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

fn build_codes(input_device_classes: &[InputDeviceClass]) -> HashMap<Box<str>, Box<str>> {
	input_device_classes
		.iter()
		.flat_map(|device_class| {
			let device_class_name = match (&device_class.name, device_class.devices.len() > 1) {
				(InputDeviceClassName::Keyboard, false) => None,
				(InputDeviceClassName::Keyboard, true) => Some("Kbd"),
				(InputDeviceClassName::Joystick, _) => Some("Joy"),
				(InputDeviceClassName::Lightgun, _) => Some("Gun"),
				(InputDeviceClassName::Mouse, _) => Some("Mouse"),
				(InputDeviceClassName::Other(x), _) => Some(x.as_ref()),
			};
			device_class
				.devices
				.iter()
				.map(move |device| (device_class_name, device))
		})
		.flat_map(|(device_class_name, device)| {
			device
				.items
				.iter()
				.map(move |item| (device_class_name, device.devindex, item))
		})
		.map(|(device_class_name, device_index, item)| {
			let label = if let Some(device_class_name) = device_class_name {
				format!("{} #{} {}", device_class_name, device_index + 1, item.name).into()
			} else {
				item.name.as_str().into()
			};
			(item.code.as_str().into(), label)
		})
		.collect::<HashMap<_, _>>()
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

		InputCluster::Multi {
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
		InputCluster::Multi {
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

fn input_cluster_code_text(
	inputs: &[Input],
	cluster: &InputCluster,
	codes: &HashMap<Box<str>, impl AsRef<str>>,
) -> String {
	let seqs_iter = match cluster {
		InputCluster::Single(index) => {
			let seqs_iter = inputs[*index].seq_standard_tokens.as_deref().into_iter();
			Either::Left(seqs_iter)
		}
		InputCluster::Multi {
			x_input_index,
			y_input_index,
			..
		} => {
			let seqs_iter = [*x_input_index, *y_input_index]
				.into_iter()
				.flatten()
				.map(|index| &inputs[index])
				.flat_map(|input| {
					[
						input.seq_decrement_tokens.as_deref(),
						input.seq_increment_tokens.as_deref(),
					]
				})
				.flatten();
			Either::Right(seqs_iter)
		}
	};

	seqs_iter
		.map(|seq_tokens| seq_tokens_desc_from_string(seq_tokens, codes))
		.join(" / ")
}

fn input_cluster_as_multi(input_cluster: &InputCluster) -> Option<(Option<usize>, Option<usize>, Option<&'_ str>)> {
	match input_cluster {
		InputCluster::Single(_) => None,
		InputCluster::Multi {
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

	let result = InputCluster::Multi {
		x_input_index,
		y_input_index,
		aggregate_name,
	};
	Some(result)
}

fn seq_tokens_desc_from_string(s: &str, codes: &HashMap<Box<str>, impl AsRef<str>>) -> String {
	seq_tokens_from_string(s)
		.flat_map(|token| match token {
			SeqToken::Named(text, modifier) => {
				let text = codes.get(text).map(|x| x.as_ref()).unwrap_or(text);
				match modifier {
					None => vec![text],
					Some(SeqTokenModifier::LeftSwitch) => vec![text, "Left"],
					Some(SeqTokenModifier::UpSwitch) => vec![text, "Up"],
					Some(SeqTokenModifier::RightSwitch) => vec![text, "Right"],
					Some(SeqTokenModifier::DownSwitch) => vec![text, "Down"],
					Some(SeqTokenModifier::Relative) => vec![text, "Relative"],
					Some(SeqTokenModifier::Absolute) => vec![text, "Absolute"],
					Some(SeqTokenModifier::Unknown(modifier)) => vec![text, modifier],
				}
			}
			SeqToken::Or => vec!["or"],
			SeqToken::Not => vec!["not"],
			SeqToken::Default => vec!["default"],
		})
		.join(" ")
}

fn seq_tokens_from_string(s: &str) -> impl Iterator<Item = SeqToken<'_>> {
	s.split(' ')
		.map(|token_text| {
			// binary tokens like OR/NOT/DEFAULT should just be "lowercased" (this
			// will need to be reevaluated when it is time to localize this)
			match token_text {
				"OR" => SeqToken::Or,
				"NOT" => SeqToken::Not,
				"DEFAULT" => SeqToken::Default,
				_ => {
					// here we need to split tokens into their base and modifier, like this:
					//
					//  KEYCODE_0                       ==> "KEYCODE_0", ""
					//  JOYCODE_1_BUTTON1               ==> "JOYCODE_1_BUTTON1", ""
					//  JOYCODE_1_XAXIS                 ==> "JOYCODE_1_XAXIS", ""
					//  JOYCODE_1_XAXIS_RIGHT_SWITCH    ==> "JOYCODE_1_XAXIS", "RIGHT_SWITCH"
					//
					// first step is to iterate over the first two words, or three if the
					// second word is numeric
					let mut sep_iter = token_text.match_indices('_').map(|x| x.0);
					let sep_1_pos = sep_iter.next();
					let sep_2_pos = sep_iter.next();

					// now find the index of the modifier sep
					let sep_modifier_pos = if Option::zip(sep_1_pos, sep_2_pos).is_some_and(|(sep_1_pos, sep_2_pos)| {
						token_text[(sep_1_pos + 1)..sep_2_pos]
							.chars()
							.all(|ch| ch.is_ascii_digit())
					}) {
						sep_iter.next()
					} else {
						sep_2_pos
					};

					// parse out the base and modifier
					let (base, modifier) = if let Some(sep_modifier_pos) = sep_modifier_pos {
						(&token_text[..sep_modifier_pos], &token_text[(sep_modifier_pos + 1)..])
					} else {
						(token_text, "")
					};

					// interpret the modifier
					let modifier = (!modifier.is_empty())
						.then(|| SeqTokenModifier::from_str(modifier).unwrap_or(SeqTokenModifier::Unknown(modifier)));

					// and return
					SeqToken::Named(base, modifier)
				}
			}
		})
		.skip_while(|token| *token == SeqToken::Or)
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use super::SeqToken;
	use super::SeqToken::Named;
	use super::SeqToken::Or;
	use super::SeqTokenModifier::RightSwitch;

	#[test_case(0, "KEYCODE_0", &[Named("KEYCODE_0", None)])]
	#[test_case(1, "KEYCODE_0 OR KEYCODE_1", &[Named("KEYCODE_0", None), Or, Named("KEYCODE_1", None)])]
	#[test_case(2, "OR KEYCODE_A OR KEYCODE_B", &[Named("KEYCODE_A", None), Or, Named("KEYCODE_B", None)])]
	#[test_case(3, "JOYCODE_1_BUTTON1", &[Named("JOYCODE_1_BUTTON1", None)])]
	#[test_case(4, "JOYCODE_1_XAXIS", &[Named("JOYCODE_1_XAXIS", None)])]
	#[test_case(5, "JOYCODE_1_XAXIS_RIGHT_SWITCH", &[Named("JOYCODE_1_XAXIS", Some(RightSwitch))])]
	fn seq_tokens_from_string(_index: usize, s: &str, expected: &[SeqToken<'_>]) {
		let actual = super::seq_tokens_from_string(s).collect::<Vec<_>>();
		assert_eq!(expected, actual.as_slice());
	}
}
