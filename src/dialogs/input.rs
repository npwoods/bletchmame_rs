use std::borrow::Cow;
use std::collections::HashMap;

use itertools::Itertools;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::ModelRc;
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
use crate::ui::InputDialog;
use crate::ui::InputDialogEntry;

pub async fn dialog_input(
	parent: Weak<impl ComponentHandle + 'static>,
	inputs: impl AsRef<[Input]>,
	input_device_classes: impl AsRef<[InputDeviceClass]>,
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
		.map(|x| {
			let name = SharedString::from(&x.name);
			let seq_tokens = x.seq_standard_tokens.as_deref().unwrap_or_default();
			let text = seq_text_from_tokens(seq_tokens, &codes).into();
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

fn seq_text_from_tokens(seq_tokens: &str, codes: &HashMap<&str, Cow<str>>) -> String {
	#[derive(Debug, strum::Display, EnumString, PartialEq)]
	enum Token<'a> {
		#[strum(to_string = "{0}")]
		Named(&'a str),
		#[strum(to_string = "or")]
		Or,
		#[strum(to_string = "not")]
		Not,
		#[strum(to_string = "default")]
		Default,
	}

	seq_tokens
		.split(' ')
		.map(|token| {
			// modifier tokens like OR/NOT/DEFAULT should just be "lowercaseed" (this
			// will need to be reevaluated when it is time to localize this)
			match token {
				"OR" => Token::Or,
				"NOT" => Token::Not,
				"DEFAULT" => Token::Default,
				token => {
					let text = codes.get(token).map(|x| x.as_ref()).unwrap_or(token);
					Token::Named(text)
				}
			}
		})
		.skip_while(|token| *token == Token::Or)
		.map(|x| x.to_string())
		.join(" ")
}
