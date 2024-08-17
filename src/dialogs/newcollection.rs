use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::Weak;
use tokio::sync::Notify;

use crate::guiutils::windowing::with_modal_parent;
use crate::ui::NewCollectionDialog;

pub async fn dialog_new_collection(
	parent: Weak<impl ComponentHandle + 'static>,
	existing_names: Vec<String>,
) -> Option<String> {
	// prepare the dialog
	let dialog = with_modal_parent(&parent.unwrap(), || NewCollectionDialog::new().unwrap());
	let notify = Rc::new(Notify::new());
	let result = Rc::new(RefCell::new(None));

	// set the intial name
	let new_collection_name = create_new_name(&existing_names);
	dialog.set_text(new_collection_name.as_ref().into());

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

	// set up the close handler
	let notify_clone = notify.clone();
	dialog.window().on_close_requested(move || {
		notify_clone.notify_one();
		CloseRequestResponse::HideWindow
	});

	// we want the "ok" button to be disabled when bad names are proposed
	let dialog_weak = dialog.as_weak();
	dialog.on_text_edited(move |new_name| {
		let ok_enabled = is_good_new_name(&existing_names, &new_name);
		dialog_weak.unwrap().set_ok_enabled(ok_enabled);
	});

	// show the dialog and wait for completion
	dialog.show().unwrap();
	notify.notified().await;
	dialog.hide().unwrap();

	Rc::unwrap_or_clone(result).into_inner()
}

fn create_new_name(existing_names: &[String]) -> Cow<'static, str> {
	let mut count = 1u32;
	loop {
		let new_name: Cow<str> = if count > 1 {
			format!("New Collection {count}").into()
		} else {
			"New Collection".into()
		};
		if is_good_new_name(existing_names, &new_name) {
			break new_name;
		}
		count += 1;
	}
}

fn is_good_new_name(existing_names: &[String], new_name: &str) -> bool {
	!new_name.is_empty() && !existing_names.iter().any(|x| x.eq(new_name))
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	#[test_case(0, &[], "", false)]
	#[test_case(1, &[], "Foo", true)]
	#[test_case(2, &[], "New Collection", true)]
	#[test_case(3, &["New Collection"], "New Collection", false)]
	#[test_case(4, &["New Collection"], "New Collection 2", true)]
	fn is_good_new_name(_index: usize, existing_names: &[&str], new_name: &str, expected: bool) {
		let existing_names = existing_names.iter().map(|x| x.to_string()).collect::<Vec<_>>();
		let actual = super::is_good_new_name(&existing_names, new_name);
		assert_eq!(expected, actual);
	}
}
