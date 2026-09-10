use slint::CloseRequestResponse;
use slint::ComponentHandle;
use tokio::sync::mpsc;

use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::prefs::PrefsVideo;
use crate::ui::VideoDialog;
use crate::videoconfig::VideoConfigInfo;

pub async fn dialog_video(modal_stack: ModalStack, video: PrefsVideo) -> Option<PrefsVideo> {
	let modal = modal_stack.modal(|| VideoDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);
	let config_info = VideoConfigInfo::new();

	// set up the video settings
	let settings = config_info.to_ui_video_settings(&video);
	let default_settings = config_info.to_ui_video_settings(&PrefsVideo::default());
	modal.dialog().set_settings(settings.clone());
	modal.dialog().set_original_settings(settings);
	modal.dialog().set_default_settings(default_settings);
	modal.dialog().set_config_info(config_info.ui_config_info.clone());

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
		let results = config_info.from_ui_video_settings(&results);
		tx_clone.signal(Some(results));
	});

	// show the dialog
	modal.run(async { rx.recv().await.unwrap() }).await
}
