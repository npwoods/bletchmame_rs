use std::borrow::Cow;
use std::default::Default;

use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::SharedString;

use crate::dialogs::SingleResult;
use crate::guiutils::modal::ModalStack;
use crate::ui::NameCollectionDialog;

async fn dialog_name_collection(
	modal_stack: ModalStack,
	title: impl Into<SharedString>,
	existing_names: Vec<String>,
	default_name: impl Into<SharedString>,
) -> Option<String> {
	// prepare the dialog
	let modal = modal_stack.modal(|| NameCollectionDialog::new().unwrap());
	let single_result = SingleResult::default();

	// set the title and default name
	modal.dialog().set_title_text(title.into());
	modal.dialog().invoke_set_text(default_name.into());

	// set up the "ok" button
	let signaller = single_result.signaller();
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_ok_clicked(move || {
		let text = dialog_weak.unwrap().get_text().to_string();
		signaller.signal(Some(text));
	});

	// set up the "cancel" button
	let signaller = single_result.signaller();
	modal.dialog().on_cancel_clicked(move || {
		signaller.signal(None);
	});

	// set up the close handler
	let signaller = single_result.signaller();
	modal.window().on_close_requested(move || {
		signaller.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// we want the "ok" button to be disabled when bad names are proposed
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_text_edited(move |new_name| {
		let ok_enabled = is_good_new_name(&existing_names, &new_name);
		dialog_weak.unwrap().set_ok_enabled(ok_enabled);
	});

	// show the dialog and wait for completion
	modal.run(async { single_result.wait().await }).await
}

pub async fn dialog_new_collection(modal_stack: ModalStack, existing_names: Vec<String>) -> Option<String> {
	let default_name = create_new_name(&existing_names);
	let title = "Create New Collection";
	dialog_name_collection(modal_stack, title, existing_names, default_name.as_ref()).await
}

fn create_new_name(existing_names: &[String]) -> impl AsRef<str> + use<> {
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

pub async fn dialog_rename_collection(
	modal_stack: ModalStack,
	existing_names: Vec<String>,
	old_name: String,
) -> Option<String> {
	let title = format!("Rename Folder \"{}\"", old_name);
	dialog_name_collection(modal_stack, title, existing_names, old_name).await
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
