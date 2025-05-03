use std::default::Default;
use std::fmt::Display;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use slint::Weak;
use strum::VariantArray;

use crate::dialogs::SingleResult;
use crate::guiutils::modal::Modal;
use crate::ui::MessageBoxDialog;

pub trait MessageBoxDefaults: VariantArray {
	fn accept() -> Self;
	fn abort() -> Self;
}

#[derive(Debug, Clone, Copy, PartialEq, VariantArray, strum::Display)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, VariantArray, strum::Display)]
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
	let values = T::VARIANTS;
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
	let modal = Modal::new(&parent.unwrap(), || MessageBoxDialog::new().unwrap());
	let single_result = SingleResult::default();

	// turn value_texts into a model
	let value_texts = VecModel::from(value_texts);
	let value_texts = ModelRc::new(value_texts);

	// set the dialog properties
	modal.dialog().set_title_text(title);
	modal.dialog().set_message_text(message);
	modal.dialog().set_button_texts(value_texts);

	// set button callbacks
	let signaller = single_result.signaller();
	modal.dialog().on_button_clicked(move |index| {
		let index = usize::try_from(index).unwrap();
		signaller.signal(index)
	});

	// close requested callback
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(abort_index);
		CloseRequestResponse::KeepWindowShown
	});

	// show the dialog and wait for completion
	modal.run(async { single_result.wait().await }).await
}
