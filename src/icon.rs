use slint::Global;
use slint::Image;

use crate::ui::Icons;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Icon {
	#[default]
	Blank,
	Clear,
	Folder,
	Search,
}

impl Icon {
	pub fn slint_icon<T>(self, component: &T) -> Image
	where
		for<'a> Icons<'a>: Global<'a, T>,
	{
		let icons = Icons::get(component);
		match self {
			Icon::Blank => Image::default(),
			Icon::Clear => icons.get_clear(),
			Icon::Folder => icons.get_folder(),
			Icon::Search => icons.get_search(),
		}
	}
}
