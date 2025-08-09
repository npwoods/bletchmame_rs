use rfd::AsyncFileDialog;
use smol_str::SmolStr;

use crate::guiutils::modal::ModalStack;
use crate::imagedesc::ImageDesc;

#[derive(Clone, Debug)]
pub struct Format {
	pub description: SmolStr,
	pub extensions: Vec<SmolStr>,
}

pub async fn dialog_load_image(modal_stack: ModalStack, formats: &[Format]) -> Option<ImageDesc> {
	let all_extensions = formats.iter().flat_map(|f| f.extensions.clone()).collect::<Vec<_>>();

	let parent = modal_stack.top();
	let dialog = AsyncFileDialog::new();
	let dialog = dialog.set_parent(&parent);
	let dialog = dialog.add_filter("All Formats", &all_extensions);
	let dialog = formats.iter().fold(dialog, |dialog, fmt| {
		dialog.add_filter(fmt.description.clone(), &fmt.extensions)
	});

	let filename = dialog.pick_file().await?.path().to_str()?.into();
	Some(ImageDesc::File(filename))
}
