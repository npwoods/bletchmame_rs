use std::fmt::Display;

use slint::CloseRequestResponse;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use strum::VariantArray;
use tokio::sync::mpsc;

use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
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
	modal_stack: ModalStack,
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
	let value_texts = values.iter().map(|x| x.to_string().into()).collect::<Vec<_>>();

	// determine accept/abort indexes
	let accept_index = values.iter().position(|x| *x == T::accept()).unwrap();
	let abort_index = values.iter().position(|x| *x == T::abort()).unwrap();

	// and run it
	let result_index =
		internal_dialog_message_box(modal_stack, title, message, value_texts, accept_index, abort_index).await;
	values[result_index].clone()
}

async fn internal_dialog_message_box(
	modal_stack: ModalStack,
	title: SharedString,
	message: SharedString,
	value_texts: Vec<SharedString>,
	_accept_index: usize,
	abort_index: usize,
) -> usize {
	// prepare the dialog
	let modal = modal_stack.modal(|| MessageBoxDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// turn value_texts into a model
	let value_texts = VecModel::from(value_texts);
	let value_texts = ModelRc::new(value_texts);

	// set the dialog properties
	modal.dialog().set_title_text(title);
	modal.dialog().set_message_text(message);
	modal.dialog().set_button_texts(value_texts);

	// set button callbacks
	let tx_clone = tx.clone();
	modal.dialog().on_button_clicked(move |index| {
		let index = usize::try_from(index).unwrap();
		tx_clone.signal(index)
	});

	// close requested callback
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(abort_index);
		CloseRequestResponse::KeepWindowShown
	});

	// show the dialog and wait for completion
	modal.run(async { rx.recv().await.unwrap() }).await
}
