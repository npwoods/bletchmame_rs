use rfd::FileDialog;
use slint::ComponentHandle;
use slint::Weak;

use crate::status::Image;

pub fn dialog_load_image(_parent: Weak<impl ComponentHandle + 'static>, image: &Image) -> Option<String> {
	let dialog = FileDialog::new();
	let all_extensions = image
		.details
		.formats
		.iter()
		.flat_map(|f| &f.extensions)
		.collect::<Vec<_>>();
	let dialog = dialog.add_filter("All Formats", &all_extensions);

	let dialog = image.details.formats.iter().fold(dialog, |dialog, fmt| {
		dialog.add_filter(fmt.description.clone(), &fmt.extensions)
	});

	let filename = dialog.pick_file()?;
	let filename = filename.into_os_string().into_string().unwrap();
	Some(filename)
}
