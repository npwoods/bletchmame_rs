use easy_ext::ext;

use crate::info::Machine;
use crate::software::Software;
use crate::ui::InfoDisplay;

#[ext(InfoDisplayExt)]
pub impl InfoDisplay {
	fn from_machine(machine: &Machine) -> Self {
		Self {
			name: machine.name().into(),
			source_file: machine.source_file().into(),
			description: machine.description().into(),
			provider: machine.manufacturer().into(),
			year: machine.year().into(),
			status: machine.driver_status().to_string().into(),
		}
	}

	fn from_software(software_list_name: impl AsRef<str>, software: &Software) -> Self {
		Self {
			name: software.name.as_str().into(),
			source_file: software_list_name.as_ref().into(),
			description: software.description.as_str().into(),
			provider: software.publisher.as_str().into(),
			year: software.year.as_str().into(),
			status: "".into(),
		}
	}

	fn brief(self) -> Self {
		Self {
			description: self.description,
			provider: self.provider,
			status: self.status,
			..Default::default()
		}
	}
}
