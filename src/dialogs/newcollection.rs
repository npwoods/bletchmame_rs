use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::Weak;
use tokio::sync::Notify;

use crate::guiutils::windowing::with_modal_parent;
use crate::ui::NewCollectionDialog;

pub async fn dialog_new_collection(
	parent: Weak<impl ComponentHandle + 'static>,
	_existing_names: Vec<String>,
) -> Option<String> {
	// prepare the dialog
	let dialog = with_modal_parent(&parent.unwrap(), || NewCollectionDialog::new().unwrap());
	let notify = Rc::new(Notify::new());
	let result = Rc::new(RefCell::new(None));

	// set up the "ok" button
	let notify_clone = notify.clone();
	let result_clone = result.clone();
	let dialog_weak = dialog.as_weak();
	dialog.on_ok_clicked(move || {
		let text = dialog_weak.unwrap().get_text().to_string();
		result_clone.replace(Some(text));
		notify_clone.notify_one();
	});

	// set up the "cancel" button
	let notify_clone = notify.clone();
	dialog.on_cancel_clicked(move || {
		notify_clone.notify_one();
	});

	// show the dialog and wait for completion
	dialog.show().unwrap();
	notify.notified().await;
	dialog.hide().unwrap();

	Rc::unwrap_or_clone(result).into_inner()
}
