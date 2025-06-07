use rfd::FileDialog;

use crate::guiutils::modal::ModalStack;

#[derive(Clone, Debug)]
pub struct Format<'a> {
	pub description: &'a str,
	pub extensions: &'a [String],
}

pub fn dialog_load_image<'a>(
	_modal_stack: ModalStack,
	format_iter: impl Iterator<Item = Format<'a>> + Clone,
) -> Option<String> {
	let all_extensions = format_iter.clone().flat_map(|f| f.extensions).collect::<Vec<_>>();

	let dialog = FileDialog::new();
	let dialog = dialog.add_filter("All Formats", &all_extensions);
	let dialog = format_iter.fold(dialog, |dialog, fmt| dialog.add_filter(fmt.description, fmt.extensions));

	let filename = dialog.pick_file()?;
	let filename = filename.into_os_string().into_string().unwrap();
	Some(filename)
}
