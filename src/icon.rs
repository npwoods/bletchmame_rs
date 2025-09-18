use slint::Global;
use slint::Image;

use crate::ui::Icons;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Icon {
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
			Icon::Folder => icons.get_folder(),
			Icon::Search => icons.get_search(),
		}
	}
}
