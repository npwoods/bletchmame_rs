use std::any::Any;
use std::cell::RefCell;

use slint::Model;
use slint::ModelNotify;
use slint::ModelRc;
use slint::ModelTracker;
use slint::SharedString;
use slint::VecModel;

use crate::devimageconfig::DevicesImagesConfig;
use crate::devimageconfig::EntryDetails;
use crate::imagedesc::ImageDesc;
use crate::ui::DeviceAndImageEntry;

pub struct DevicesAndImagesModel {
	diconfig: RefCell<DevicesImagesConfig>,
	pub none_string: SharedString,
	notify: ModelNotify,
}

impl DevicesAndImagesModel {
	pub fn new(diconfig: DevicesImagesConfig) -> Self {
		let none_string = SharedString::from("<<none>>");
		Self {
			diconfig: RefCell::new(diconfig),
			none_string: none_string.clone(),
			notify: ModelNotify::default(),
		}
	}

	pub fn change_diconfig(&self, callback: impl FnOnce(&DevicesImagesConfig) -> Option<DevicesImagesConfig>) {
		// update the config in our RefCell
		let range = {
			let mut diconfig = self.diconfig.borrow_mut();
			let new_diconfig = callback(&diconfig);
			if let Some(new_diconfig) = new_diconfig {
				let range = diconfig.identify_changed_rows(&new_diconfig);
				*diconfig = new_diconfig;
				range
			} else {
				Some(Vec::new())
			}
		};

		// notify row changes (if any)
		if let Some(range) = range {
			for row in range {
				self.notify.row_changed(row);
			}
		} else {
			self.notify.reset();
		}
	}

	pub fn with_diconfig<R>(&self, callback: impl FnOnce(&DevicesImagesConfig) -> R) -> R {
		let diconfig = self.diconfig.borrow();
		callback(&diconfig)
	}

	pub fn get_model(model: &impl Model) -> &'_ Self {
		model.as_any().downcast_ref::<DevicesAndImagesModel>().unwrap()
	}

	pub fn set_slot_entry_option(&self, entry_index: usize, new_option_name: Option<&str>) {
		self.change_diconfig(|diconfig| {
			let tag = diconfig.entry(entry_index).unwrap().tag;
			Some(diconfig.set_slot_option(tag, new_option_name))
		})
	}
}

impl Model for DevicesAndImagesModel {
	type Data = DeviceAndImageEntry;

	fn row_count(&self) -> usize {
		self.diconfig.borrow().entry_count()
	}

	fn row_data(&self, row: usize) -> Option<Self::Data> {
		// retrieve the entry
		let diconfig = self.diconfig.borrow();
		let entry = diconfig.entry(row)?;

		// basic indent and display tag stuff
		let display_tag = SharedString::from(entry.subtag);
		let indent = entry.indent.try_into().unwrap();

		// now figure out the slot/image-specific details
		let (options, current_option_index, filename) = match entry.details {
			EntryDetails::Slot {
				options,
				current_option_index,
			} => {
				let options = options
					.into_iter()
					.map(|opt| {
						if let Some(name) = opt.name {
							let name = SharedString::from(name.as_ref());
							if let Some(desc) = opt.description {
								(name.clone(), format!("{desc} ({name})").into())
							} else {
								(name.clone(), name)
							}
						} else {
							("".into(), self.none_string.clone())
						}
					})
					.collect::<Vec<_>>();
				let current_option_index = current_option_index.try_into().unwrap();
				(options, current_option_index, "".into())
			}
			EntryDetails::Image { image_desc } => {
				let display_name = image_desc
					.map(ImageDesc::display_name)
					.unwrap_or_default()
					.as_ref()
					.into();
				(Vec::default(), -1, display_name)
			}
		};

		let (option_names, option_descriptions): (Vec<_>, Vec<_>) = options.into_iter().unzip();
		let option_names = VecModel::from(option_names);
		let option_names = ModelRc::new(option_names);
		let option_descriptions = VecModel::from(option_descriptions);
		let option_descriptions = ModelRc::new(option_descriptions);

		let result = DeviceAndImageEntry {
			display_tag,
			indent,
			option_names,
			option_descriptions,
			current_option_index,
			filename,
		};
		Some(result)
	}

	fn model_tracker(&self) -> &dyn ModelTracker {
		&self.notify
	}

	fn as_any(&self) -> &dyn Any {
		self
	}
}
