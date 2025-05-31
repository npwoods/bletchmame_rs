use std::borrow::Cow;
use std::collections::HashMap;
use std::str::FromStr;

use itertools::Either;
use itertools::Itertools;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::ModelRc;
use slint::VecModel;
use slint::Weak;
use strum::EnumString;

use crate::dialogs::SingleResult;
use crate::guiutils::modal::Modal;
use crate::status::Input;
use crate::status::InputClass;
use crate::status::InputDeviceClass;
use crate::status::InputDeviceClassName;
use crate::ui::InputDialog;
use crate::ui::InputDialogEntry;

#[derive(Debug)]
enum InputCluster<'a> {
	Single(&'a Input),
	Multi {
		x_input: Option<&'a Input>,
		y_input: Option<&'a Input>,
		aggregate_name: Option<&'a str>,
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
	inputs: impl AsRef<[Input]>,
	input_device_classes: impl AsRef<[InputDeviceClass]> + '_,
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

	// build the codes map
	let codes = build_codes(input_device_classes.as_ref());

	// set up entries
	let entries = inputs
		.as_ref()
		.iter()
		.filter(move |x| x.class == Some(class))
		.sorted_by_key(|input| {
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
			if a.port_tag == b.port_tag && a.mask == b.mask {
				Ok(a)
			} else {
				Err((a, b))
			}
		})
		.map(input_cluster_from_input)
		.coalesce(|a, b| coalesce_input_clusters(&a, &b).ok_or((a, b)))
		.map(|input_cluster| {
			let name = input_cluster_name(&input_cluster).into();
			let text = input_cluster_code_text(&input_cluster, &codes).into();
			InputDialogEntry { name, text }
		})
		.collect::<Vec<_>>();
	let entries = VecModel::from(entries);
	let entries = ModelRc::new(entries);
	modal.dialog().set_entries(entries);

	// present the modal dialog
	modal.run(async { single_result.wait().await }).await
}

fn build_codes(input_device_classes: &[InputDeviceClass]) -> HashMap<&'_ str, Cow<str>> {
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
				Cow::Owned(format!("{} #{} {}", device_class_name, device_index + 1, item.name))
			} else {
				Cow::Borrowed(item.name.as_str())
			};
			(item.code.as_str(), label)
		})
		.collect::<HashMap<_, _>>()
}

fn input_cluster_from_input<'a>(input: &'a Input) -> InputCluster<'a> {
	if input.is_analog {
		let name = input
			.name
			.trim_end_matches(|ch: char| ch.is_ascii_digit() || ch.is_whitespace());
		let (x_input, y_input) = if name.ends_with('Y') {
			(None, Some(input))
		} else {
			(Some(input), None)
		};

		let aggregate_name = name
			.strip_suffix(['X', 'Y', 'Z'])
			.map(|name| name.trim_end_matches(char::is_whitespace));

		InputCluster::Multi {
			x_input,
			y_input,
			aggregate_name,
		}
	} else {
		InputCluster::Single(input)
	}
}

fn input_cluster_as_multi<'a>(
	input_cluster: &InputCluster<'a>,
) -> Option<(Option<&'a Input>, Option<&'a Input>, Option<&'a str>)> {
	match input_cluster {
		InputCluster::Single(_) => None,
		InputCluster::Multi {
			x_input,
			y_input,
			aggregate_name,
		} => Some((*x_input, *y_input, *aggregate_name)),
	}
}

fn input_cluster_name<'a>(input_cluster: &InputCluster<'a>) -> &'a str {
	match input_cluster {
		InputCluster::Single(input) => &input.name,
		InputCluster::Multi {
			x_input,
			y_input,
			aggregate_name,
		} => aggregate_name
			.or_else(|| x_input.map(|i| i.name.as_str()))
			.or_else(|| y_input.map(|i| i.name.as_str()))
			.unwrap(),
	}
}

fn input_cluster_code_text(input_cluster: &InputCluster<'_>, codes: &HashMap<&'_ str, impl AsRef<str>>) -> String {
	let seqs_iter = match input_cluster {
		InputCluster::Single(input) => {
			let seqs_iter = input.seq_standard_tokens.as_deref().into_iter();
			Either::Left(seqs_iter)
		}
		InputCluster::Multi { x_input, y_input, .. } => {
			let seqs_iter = [*x_input, *y_input]
				.into_iter()
				.flatten()
				.flat_map(|i| [i.seq_decrement_tokens.as_deref(), i.seq_increment_tokens.as_deref()])
				.flatten();
			Either::Right(seqs_iter)
		}
	};

	seqs_iter
		.map(|seq_tokens| seq_tokens_desc_from_string(seq_tokens, codes))
		.join(" / ")
}

fn coalesce_input_clusters<'a>(a: &InputCluster<'a>, b: &InputCluster<'a>) -> Option<InputCluster<'a>> {
	let (a_x_input, a_y_input, a_aggregate_name) = input_cluster_as_multi(a)?;
	let (b_x_input, b_y_input, b_aggregate_name) = input_cluster_as_multi(b)?;
	if a_aggregate_name != b_aggregate_name {
		return None;
	}

	let (x_input, y_input) = match (a_x_input, a_y_input, b_x_input, b_y_input) {
		(Some(x_input), None, None, Some(y_input)) => Some((x_input, y_input)),
		(None, Some(x_input), Some(y_input), None) => Some((x_input, y_input)),
		_ => None,
	}?;
	let x_input = Some(x_input);
	let y_input = Some(y_input);
	let aggregate_name = a_aggregate_name;

	let result = InputCluster::Multi {
		x_input,
		y_input,
		aggregate_name,
	};
	Some(result)
}

fn seq_tokens_desc_from_string(s: &str, codes: &HashMap<&'_ str, impl AsRef<str>>) -> String {
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
