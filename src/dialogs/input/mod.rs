pub mod multi;
pub mod primary;
pub mod xy;

use std::borrow::Cow;
use std::collections::HashMap;
use std::str::FromStr;

use easy_ext::ext;
use itertools::Itertools;
use slint::ModelRc;
use slint::VecModel;
use strum::EnumString;

use crate::appcommand::AppCommand;
use crate::dialogs::seqpoll::SeqPollDialogType;
use crate::runtime::command::MameCommand;
use crate::runtime::command::SeqType;
use crate::status::Input;
use crate::status::InputDevice;
use crate::status::InputDeviceClass;
use crate::status::InputDeviceClassName;
use crate::status::InputDeviceItem;
use crate::ui::SimpleMenuEntry;

#[derive(Copy, Clone, Debug)]
enum InputAxis {
	X,
	Y,
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

#[ext(InputDeviceClassExt)]
pub impl InputDeviceClass {
	fn prefix(&self) -> Option<&'_ str> {
		match (&self.name, self.devices.len() > 1) {
			(InputDeviceClassName::Keyboard, false) => None,
			(InputDeviceClassName::Keyboard, true) => Some("Kbd"),
			(InputDeviceClassName::Joystick, _) => Some("Joy"),
			(InputDeviceClassName::Lightgun, _) => Some("Gun"),
			(InputDeviceClassName::Mouse, _) => Some("Mouse"),
			(InputDeviceClassName::Other(x), _) => Some(x.as_ref()),
		}
	}
}

#[ext(InputDeviceClassSliceExt)]
pub impl [InputDeviceClass] {
	fn iter_devices(&self) -> impl Iterator<Item = (&'_ InputDeviceClass, &'_ InputDevice)> {
		self.iter()
			.flat_map(|device_class| device_class.devices.iter().map(move |device| (device_class, device)))
	}

	fn iter_device_items(&self) -> impl Iterator<Item = (&'_ InputDeviceClass, &'_ InputDevice, &'_ InputDeviceItem)> {
		self.iter_devices()
			.flat_map(|(device_class, device)| device.items.iter().map(move |item| (device_class, device, item)))
	}
}

#[ext(InputSeqExt)]
pub impl AppCommand {
	fn seq_specify_dialog(input: &Input, seq_type: SeqType) -> Self {
		AppCommand::SeqPollDialog {
			port_tag: input.port_tag.clone(),
			mask: input.mask,
			seq_type,
			poll_type: SeqPollDialogType::Specify,
		}
	}

	fn seq_add_dialog(input: &Input, seq_type: SeqType) -> Self {
		AppCommand::SeqPollDialog {
			port_tag: input.port_tag.clone(),
			mask: input.mask,
			seq_type,
			poll_type: SeqPollDialogType::Add,
		}
	}

	fn seq_clear(input: &Input, seq_type: SeqType) -> Self {
		let seqs = &[(&input.port_tag, input.mask, seq_type, "")];
		MameCommand::seq_set(seqs).into()
	}

	#[allow(clippy::too_many_arguments)]
	fn set_multi_seq(
		x_input: Option<&Input>,
		y_input: Option<&Input>,
		x_standard_tokens: &str,
		x_decrement_tokens: &str,
		x_increment_tokens: &str,
		y_standard_tokens: &str,
		y_decrement_tokens: &str,
		y_increment_tokens: &str,
	) -> AppCommand {
		let seqs = [
			x_input.map(|x_input| (&x_input.port_tag, x_input.mask, SeqType::Standard, x_standard_tokens)),
			x_input.map(|x_input| (&x_input.port_tag, x_input.mask, SeqType::Decrement, x_decrement_tokens)),
			x_input.map(|x_input| (&x_input.port_tag, x_input.mask, SeqType::Increment, x_increment_tokens)),
			y_input.map(|y_input| (&y_input.port_tag, y_input.mask, SeqType::Standard, y_standard_tokens)),
			y_input.map(|y_input| (&y_input.port_tag, y_input.mask, SeqType::Decrement, y_decrement_tokens)),
			y_input.map(|y_input| (&y_input.port_tag, y_input.mask, SeqType::Increment, y_increment_tokens)),
		];
		let seqs = seqs.into_iter().flatten().collect::<Vec<_>>();
		MameCommand::seq_set(seqs.as_slice()).into()
	}

	fn input_xy_dialog(x_input: Option<&Input>, y_input: Option<&Input>) -> AppCommand {
		let x_input = x_input.map(|input| (input.port_tag.clone(), input.mask));
		let y_input = y_input.map(|input| (input.port_tag.clone(), input.mask));
		AppCommand::InputXyDialog { x_input, y_input }
	}
}

fn build_code_text<'a>(
	input_seqs: impl IntoIterator<Item = (&'a Input, Option<InputAxis>, SeqType)>,
	codes: &HashMap<Box<str>, impl AsRef<str>>,
) -> Cow<'static, str> {
	let result = input_seqs
		.into_iter()
		.filter_map(|(input, axis, seq_type)| {
			let seq_tokens = match seq_type {
				SeqType::Standard => &input.seq_standard_tokens,
				SeqType::Decrement => &input.seq_decrement_tokens,
				SeqType::Increment => &input.seq_increment_tokens,
			};

			seq_tokens
				.as_deref()
				.and_then(|seq_tokens| (!seq_tokens.is_empty()).then_some((axis, seq_type, seq_tokens)))
		})
		.map(|(axis, seq_type, seq_tokens)| {
			let prefix = match (axis, seq_type) {
				(None, _) => "",
				(Some(InputAxis::X), SeqType::Standard) => "\u{2194}",
				(Some(InputAxis::X), SeqType::Decrement) => "\u{25C0}",
				(Some(InputAxis::X), SeqType::Increment) => "\u{25B6}",
				(Some(InputAxis::Y), SeqType::Standard) => "\u{2195}",
				(Some(InputAxis::Y), SeqType::Decrement) => "\u{25B2}",
				(Some(InputAxis::Y), SeqType::Increment) => "\u{25BC}",
			};
			format!("{}{}", prefix, seq_tokens_desc_from_string(seq_tokens, codes))
		})
		.join(" / ");

	if result.is_empty() {
		"None".into()
	} else {
		result.into()
	}
}

fn build_codes(input_device_classes: &[InputDeviceClass]) -> HashMap<Box<str>, Box<str>> {
	input_device_classes
		.iter_device_items()
		.map(|(device_class, device, item)| {
			let label = if let Some(prefix) = device_class.prefix() {
				format!("{} #{} {}", prefix, device.devindex + 1, item.name).into()
			} else {
				item.name.as_str().into()
			};
			(item.code.as_str().into(), label)
		})
		.collect::<HashMap<_, _>>()
}

fn build_context_menu<'a>(
	quick_item_inputs: &[Option<&'a Input>],
	quick_items: impl IntoIterator<Item = (Cow<'a, str>, Vec<(usize, SeqType, &'a str)>)>,
	specify_command: Option<AppCommand>,
	add_command: Option<AppCommand>,
	clear_command: Option<AppCommand>,
) -> (ModelRc<SimpleMenuEntry>, ModelRc<SimpleMenuEntry>) {
	// first pass on processing quick items
	let quick_items = quick_items
		.into_iter()
		.filter(|(_, seqs)| !seqs.is_empty())
		.map(|(title, seqs)| {
			let seqs = seqs
				.into_iter()
				.filter_map(|(input_index, seq_type, code)| {
					quick_item_inputs[input_index]
						.as_ref()
						.map(|input| (input, seq_type, code))
				})
				.map(|(input, seq_type, code)| (&input.port_tag, input.mask, seq_type, code))
				.collect::<Vec<_>>();
			(title, seqs)
		})
		.collect::<Vec<_>>();

	// do we need a "Multiple..." entry?
	let multiple_command = (quick_items.len() >= 2).then(|| {
		let selections = quick_items
			.iter()
			.map(|(title, seqs)| {
				let title = title.as_ref().into();
				let seqs = seqs
					.iter()
					.map(|(port_tag, mask, seq_type, code)| ((*port_tag).clone(), *mask, *seq_type, (*code).into()))
					.collect::<Vec<_>>();
				(title, seqs)
			})
			.collect::<Vec<_>>();
		let command = AppCommand::InputSelectMultipleDialog { selections };
		let command = command.encode_for_slint();
		let title = "Multiple...".into();
		SimpleMenuEntry { title, command }
	});

	// now combine
	let entries_1 = quick_items
		.into_iter()
		.map(|(title, seqs)| {
			let command = MameCommand::seq_set(&seqs);
			let command = AppCommand::from(command).encode_for_slint();
			let title = title.as_ref().into();
			SimpleMenuEntry { title, command }
		})
		.chain(multiple_command)
		.collect::<Vec<_>>();

	let entries_2 = [
		("Specify...", specify_command),
		("Add...", add_command),
		("Clear", clear_command),
	];
	let entries_2 = entries_2
		.into_iter()
		.filter_map(|(title, command)| {
			command.map(|command| {
				let title = title.into();
				let command = command.encode_for_slint();
				SimpleMenuEntry { title, command }
			})
		})
		.collect::<Vec<_>>();

	let entries_1 = VecModel::from(entries_1);
	let entries_2 = VecModel::from(entries_2);
	let entries_1 = ModelRc::new(entries_1);
	let entries_2 = ModelRc::new(entries_2);
	(entries_1, entries_2)
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
