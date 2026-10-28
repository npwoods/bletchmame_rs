use std::ops::ControlFlow;
use std::path::MAIN_SEPARATOR;
use std::path::Path;

use rfd::AsyncFileDialog;
use slint::ComponentHandle;
use slint::ModelRc;
use slint::StandardListViewItem;
use slint::VecModel;
use smol_str::SmolStr;
use strum::VariantArray as _;
use tokio::sync::mpsc;
use tracing::warn;

use crate::archive::Archive;
use crate::archive::ArchiveFormat;
use crate::dialogs::SenderExt;
use crate::guiutils::modal::ModalStack;
use crate::imagedesc::ImageDesc;
use crate::ui::ImageArchiveDialog;

#[derive(Clone, Debug)]
pub struct Format {
	pub description: SmolStr,
	pub extensions: Vec<SmolStr>,
}

pub async fn dialog_load_image(modal_stack: ModalStack, formats: &[Format]) -> Option<ImageDesc> {
	loop {
		let result = dialog_load_image_once(modal_stack.clone(), formats).await;
		if let Some(filename) = result.break_value() {
			break filename.map(|filename| ImageDesc::File(filename.into()));
		}
	}
}

async fn dialog_load_image_once(modal_stack: ModalStack, formats: &[Format]) -> ControlFlow<Option<String>> {
	// present the file dialog
	let Some(filename) = pick_file(modal_stack.clone(), formats).await else {
		return ControlFlow::Break(None);
	};
	let path = Path::new(&filename);

	// if this isn't an archive, then we're done
	if archive_format_for_path(path).is_none() {
		ControlFlow::Break(Some(filename))
	} else {
		// open the archive
		let Ok(archive) = Archive::open(path) else {
			warn!(?filename, "Failed to open archive");
			return ControlFlow::Continue(());
		};

		// and identify the entries that can be shown as images
		let entries = archive
			.list()
			.filter_map(Result::ok)
			.filter(|entry| is_supported_entry(&entry.filename, formats))
			.map(|entry| String::from(entry.filename))
			.collect::<Vec<_>>();

		// get the entry index, either by dialog or unilaterally if there's only one entry
		let entry_index = if entries.len() == 1 {
			// only one entry?  that makes the choice very easy
			ControlFlow::Break(Some(0))
		} else {
			// in other scenarios, we have to show a dialog
			dialog_image_archive(modal_stack.clone(), &filename, &entries).await
		};

		entry_index.map_break(|index| index.map(|index| format!("{}{}{}", filename, MAIN_SEPARATOR, entries[index])))
	}
}

/// Presents the "open file" dialog
async fn pick_file(modal_stack: ModalStack, formats: &[Format]) -> Option<String> {
	let all_extensions = formats
		.iter()
		.flat_map(|f| f.extensions.clone())
		.chain(ArchiveFormat::VARIANTS.iter().map(|f| f.extension().into()))
		.collect::<Vec<_>>();

	let parent = modal_stack.top();
	let dialog = AsyncFileDialog::new();
	let dialog = dialog.set_parent(&parent);
	let dialog = dialog.add_filter("All Formats", &all_extensions);
	let dialog = formats.iter().fold(dialog, |dialog, fmt| {
		dialog.add_filter(fmt.description.clone(), &fmt.extensions)
	});
	let dialog = dialog.add_filter(
		"Archive Files",
		&ArchiveFormat::VARIANTS
			.iter()
			.map(|f| f.extension())
			.collect::<Vec<_>>(),
	);

	dialog.pick_file().await?.path().to_str().map(String::from)
}

/// Determines the archive format for a path based on its file extension
fn archive_format_for_path(path: &Path) -> Option<ArchiveFormat> {
	let extension = path.extension().and_then(|x| x.to_str()).map(|x| x.to_lowercase());
	ArchiveFormat::VARIANTS
		.iter()
		.find(|format| Some(format.extension()) == extension.as_deref())
		.copied()
}

/// Determines whether a file entry (by name) has an extension supported by one of the formats
fn is_supported_entry(entry: &str, formats: &[Format]) -> bool {
	let extension = Path::new(entry).extension().and_then(|x| x.to_str());
	extension.is_some_and(|extension| {
		formats
			.iter()
			.flat_map(|f| f.extensions.iter())
			.any(|x| x.eq_ignore_ascii_case(extension))
	})
}

/// Presents the dialog for selecting a file from within an archive
async fn dialog_image_archive(
	modal_stack: ModalStack,
	image_filename: &str,
	entries: &[impl AsRef<str>],
) -> ControlFlow<Option<usize>> {
	// prepare the dialog
	let modal = modal_stack.modal(|| ImageArchiveDialog::new().unwrap());
	let (tx, mut rx) = mpsc::channel(1);

	// set the title and the list model
	modal.dialog().set_image_filename(image_filename.into());
	let list_model = entries
		.iter()
		.map(|entry| StandardListViewItem::from(entry.as_ref()))
		.collect::<VecModel<_>>();
	modal.dialog().set_list_model(ModelRc::new(list_model));

	// set up the "ok" button
	let tx_clone = tx.clone();
	let dialog_weak = modal.dialog().as_weak();
	modal.dialog().on_ok_clicked(move || {
		let selected_index = usize::try_from(dialog_weak.unwrap().get_selected_item()).unwrap();
		tx_clone.signal(ControlFlow::Break(Some(selected_index)))
	});

	// set up the "back" button
	let tx_clone = tx.clone();
	modal.dialog().on_back_clicked(move || {
		tx_clone.signal(ControlFlow::Continue(()));
	});

	// set up the close handler (treated as "cancel")
	let tx_clone = tx.clone();
	modal.window().on_close_requested(move || {
		tx_clone.signal(ControlFlow::Break(None));
		slint::CloseRequestResponse::KeepWindowShown
	});

	// present the modal dialog
	modal.run(async { rx.recv().await.unwrap() }).await
}
