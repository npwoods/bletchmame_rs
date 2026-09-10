use std::cell::RefCell;

use slint::Model;
use slint::SharedString;
use slint::ToSharedString;
use slint::VecModel;

use crate::prefs::PrefsVideo;
use crate::prefs::VideoOption;
use crate::ui;
use crate::util::IteratorExt as _;

pub struct VideoConfigInfo {
	pub ui_config_info: ui::VideoConfigInfo,
	pub video_options: RefCell<Vec<Option<VideoOption>>>,
}

impl VideoConfigInfo {
	pub fn new() -> Self {
		let video_options = vec![
			None,
			Some(VideoOption::Direct3D),
			Some(VideoOption::Bgfx),
			Some(VideoOption::Gdi),
			Some(VideoOption::OpenGL),
			Some(VideoOption::None),
		];

		let video_option_descriptions = video_options
			.iter()
			.map(|x| video_option_display_string(x.as_ref()))
			.collect_model_rc();
		let ui_config_info = ui::VideoConfigInfo {
			video_option_descriptions,
		};
		let video_options = RefCell::new(video_options);
		Self {
			ui_config_info,
			video_options,
		}
	}

	pub fn to_ui_video_settings(&self, video: &PrefsVideo) -> ui::VideoSettings {
		let video_option_index = {
			let mut video_options = self.video_options.borrow_mut();
			let video_option_index = video_options.iter().position(|x| *x == video.video_option);
			if let Some(video_option_index) = video_option_index {
				video_option_index
			} else {
				// we need to add this unknown option; add it to our list of options
				video_options.push(video.video_option.clone());

				// and add it to our list of descriptions
				self.ui_config_info
					.video_option_descriptions
					.as_any()
					.downcast_ref::<VecModel<SharedString>>()
					.unwrap()
					.push(video_option_display_string(video.video_option.as_ref()));

				// and return the index
				video_options.len() - 1
			}
		};
		let video_option_index = video_option_index.try_into().unwrap();
		let prescale = video.prescale.into();
		let extra_mame_arguments = video.extra_mame_arguments.to_shared_string();
		ui::VideoSettings {
			video_option_index,
			prescale,
			extra_mame_arguments,
		}
	}

	#[allow(clippy::wrong_self_convention)]
	pub fn from_ui_video_settings(&self, settings: &ui::VideoSettings) -> PrefsVideo {
		let video_option_index = usize::try_from(settings.video_option_index).unwrap();
		let video_option = self.video_options.borrow()[video_option_index].clone();
		let prescale = settings.prescale.try_into().unwrap();
		let extra_mame_arguments = settings.extra_mame_arguments.as_str().trim().into();
		PrefsVideo {
			video_option,
			prescale,
			extra_mame_arguments,
		}
	}
}

fn video_option_display_string(video_option: Option<&VideoOption>) -> SharedString {
	match video_option {
		None => "(default)".into(),
		Some(VideoOption::Unknown(s)) => format!("Unknown Option (\"{s}\")").into(),
		Some(x) => x.to_shared_string(),
	}
}
