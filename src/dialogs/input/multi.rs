use std::cell::Cell;
use std::fmt::Debug;
use std::rc::Rc;
use std::sync::Arc;

use itertools::Itertools;
use more_asserts::assert_ge;
use slint::CloseRequestResponse;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;
use tokio::sync::mpsc;
use tracing::info;

use crate::appcommand::AppCommand;
use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::runtime::command::MameCommand;
use crate::runtime::command::SeqType;
use crate::ui::InputSelectMultipleDialog;

pub async fn dialog_input_select_multiple(
	modal_stack: ModalStack,
	selections: impl AsRef<[(String, Vec<(Arc<str>, u32, SeqType, String)>)]> + Debug + 'static,
) -> Option<AppCommand> {
	// the sanity checks
	assert_ge!(selections.as_ref().len(), 2);
	info!(selections=?selections, "dialog_input_select_multiple");

	// set up the modal
	let modal = modal_stack.modal(|| InputSelectMultipleDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set up entries
	let entries = selections
		.as_ref()
		.iter()
		.map(|(text, _)| SharedString::from(text))
		.collect::<Vec<_>>();
	let entries = VecModel::from(entries);
	let entries = ModelRc::new(entries);
	modal.dialog().set_entries(entries);

	// set up checkbox toggled handler
	let checked = (0..selections.as_ref().len())
		.map(|_| Cell::new(false))
		.collect::<Rc<[_]>>();
	let checked_clone = checked.clone();
	modal.dialog().on_checkbox_toggled(move |index, value| {
		let index = usize::try_from(index).unwrap();
		checked_clone[index].set(value);
	});

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// set up the "cancel" button
	let tx_clone = tx.clone();
	modal.dialog().on_cancel_clicked(move || {
		tx_clone.signal(None);
	});

	// set up the "ok" button
	let tx_clone = tx.clone();
	modal.dialog().on_ok_clicked(move || {
		let selections = selections.as_ref();
		let checked = checked.as_ref();
		let result = Some(build_result(selections, checked));
		tx_clone.signal(result);
	});

	// present the modal dialog
	modal.run(async { rx.recv().await.unwrap() }).await
}

#[allow(clippy::type_complexity)]
fn build_result(selections: &[(String, Vec<(Arc<str>, u32, SeqType, String)>)], checked: &[Cell<bool>]) -> AppCommand {
	let seqs = selections
		.iter()
		.zip(checked.iter())
		.filter_map(|((_, vec), checked)| checked.get().then_some(vec.as_slice()))
		.flatten()
		.map(|(port_tag, mask, seq_type, codes)| ((port_tag.as_ref(), *mask, *seq_type), codes.as_str()))
		.into_group_map()
		.into_iter()
		.map(|(k, codes)| (k, codes.iter().join(" or ")))
		.map(|((port_tag, mask, seq_type), codes)| (port_tag, mask, seq_type, codes))
		.collect::<Vec<_>>();
	MameCommand::seq_set(&seqs).into()
}
