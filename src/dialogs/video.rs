use slint::CloseRequestResponse;
use slint::ComponentHandle;
use slint::ToSharedString;
use tokio::sync::mpsc;

use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::prefs::Preferences;
use crate::ui::VideoDialog;
use crate::ui::VideoSettings;

pub async fn dialog_video(modal_stack: ModalStack, settings: VideoSettings) -> Option<VideoSettings> {
	let modal = modal_stack.modal(|| VideoDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set up the video settings
	modal.dialog().set_settings(settings.clone());
	modal.dialog().set_original_settings(settings);
	modal.dialog().set_default_settings(default_video_settings());

	// set up the close handler
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(None);
		CloseRequestResponse::KeepWindowShown
	});

	// set up the "ok" button
	let tx_clone = tx.clone();
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_ok_clicked(move || {
		let results = dialog_weak.unwrap().get_settings();
		tx_clone.signal(Some(results));
	});

	// set up the "cancel" button
	let tx_clone = tx.clone();
	modal.dialog().on_cancel_clicked(move || {
		tx_clone.signal(None);
	});

	// show the dialog
	modal.run(async { rx.recv().await.unwrap() }).await
}

fn default_video_settings() -> VideoSettings {
	let default_prefs = Preferences::default();
	VideoSettings {
		prescale: default_prefs.prescale.into(),
		extra_mame_arguments: default_prefs.extra_mame_arguments.to_shared_string(),
	}
}
