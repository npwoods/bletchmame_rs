use std::default::Default;
use std::fmt::Display;

use derive_enum_all_values::AllValues;
use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;

use crate::dialogs::SingleResult;
use crate::guiutils::windowing::run_modal_dialog;
use crate::guiutils::windowing::with_modal_parent;
use crate::ui::MessageBoxDialog;

pub trait MessageBoxDefaults {
	fn accept() -> Self;
	fn abort() -> Self;
	fn all_values() -> &'static [Self]
	where
		Self: std::marker::Sized;
}

#[derive(Debug, Clone, Copy, PartialEq, AllValues, strum_macros::Display)]
pub enum OkOnly {
	#[strum(to_string = "OK")]
	Ok,
}

impl MessageBoxDefaults for OkOnly {
	fn accept() -> Self {
		Self::Ok
	}

	fn abort() -> Self {
		Self::Ok
	}

	fn all_values() -> &'static [Self] {
		Self::all_values()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, AllValues, strum_macros::Display)]
pub enum OkCancel {
	#[strum(to_string = "OK")]
	Ok,
	#[strum(to_string = "Cancel")]
	Cancel,
}

impl MessageBoxDefaults for OkCancel {
	fn accept() -> Self {
		Self::Ok
	}

	fn abort() -> Self {
		Self::Cancel
	}

	fn all_values() -> &'static [Self] {
		Self::all_values()
	}
}

pub async fn dialog_message_box<T>(
	parent: Weak<impl ComponentHandle + 'static>,
	title: impl Into<SharedString>,
	message: impl Into<SharedString>,
) -> T
where
	T: Display + MessageBoxDefaults + PartialEq + Clone + 'static,
{
	// normalization
	let title = title.into();
	let message = message.into();

	// get the values
	let values = T::all_values();
	let value_texts = values.iter().map(|x| format!("{}", x).into()).collect::<Vec<_>>();

	// determine accept/abort indexes
	let accept_index = values.iter().position(|x| *x == T::accept()).unwrap();
	let abort_index = values.iter().position(|x| *x == T::abort()).unwrap();

	// and run it
	let result_index =
		internal_dialog_message_box(parent, title, message, value_texts, accept_index, abort_index).await;
	values[result_index].clone()
}

async fn internal_dialog_message_box(
	parent: Weak<impl ComponentHandle + 'static>,
	title: SharedString,
	message: SharedString,
	value_texts: Vec<SharedString>,
	_accept_index: usize,
	abort_index: usize,
) -> usize {
	// prepare the dialog
	let dialog = with_modal_parent(&parent.unwrap(), || MessageBoxDialog::new().unwrap());
	let single_result = SingleResult::default();

	// turn value_texts into a model
	let value_texts = VecModel::from(value_texts);
	let value_texts = ModelRc::new(value_texts);

	// set the dialog properties
	dialog.set_title_text(title);
	dialog.set_message_text(message);
	dialog.set_button_texts(value_texts);

	// set button callbacks
	let signaller = single_result.signaller();
	dialog.on_button_clicked(move |index| {
		let index = usize::try_from(index).unwrap();
		signaller.signal(index)
	});

	// close requested callback
	let signaller = single_result.signaller();
	dialog.window().on_close_requested(move || {
		signaller.signal(abort_index);
		CloseRequestResponse::KeepWindowShown
	});

	// show the dialog and wait for completion
	run_modal_dialog(&parent.unwrap(), &dialog, async { single_result.wait().await }).await
}
