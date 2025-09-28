use std::cell::RefCell;
use std::fs::File;
use std::io::Cursor;
use std::io::ErrorKind;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use image::ImageReader;
use parallel_worker::CancelableWorker;
use parallel_worker::State;
use parallel_worker::WorkerInit;
use parallel_worker::WorkerMethods;
use slint::Image;
use slint::Rgba8Pixel;
use slint::SharedPixelBuffer;
use slint::SharedString;
use tracing::warn;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::history_xml::HistoryXml;
use crate::prefs::PrefsItem;
use crate::prefs::PrefsMachineItem;
use crate::prefs::PrefsSoftwareItem;

#[derive(Debug)]
pub enum MultiPath {
	Zip(RefCell<ZipArchive<File>>),
	Dir(PathBuf),
}

pub struct HistoryLoader(CancelableWorker<PathBuf, Result<HistoryXml>>);

pub fn snap_view_string(item: Option<&PrefsItem>) -> SharedString {
	match item {
		None => "".into(),
		Some(PrefsItem::Machine(PrefsMachineItem { machine_name, .. })) => machine_name.into(),
		Some(PrefsItem::Software(PrefsSoftwareItem {
			software_list,
			software,
			..
		})) => format!("{software_list}/{software}").into(),
	}
}

pub fn make_multi_paths(paths: &[impl AsRef<Path>]) -> Vec<MultiPath> {
	paths
		.iter()
		.filter_map(|path| match MultiPath::new(path) {
			Ok(multi_path) => Some(multi_path),
			Err(e) => {
				warn!(error=?e, path=?path.as_ref(), "MultiPath::new() returned error");
				None
			}
		})
		.collect::<Vec<_>>()
}

impl MultiPath {
	pub fn new(path: impl AsRef<Path>) -> Result<Self> {
		let path = path.as_ref();
		match File::open(path) {
			Ok(file) => Ok(Self::Zip(RefCell::new(ZipArchive::new(file)?))),
			Err(e) if e.kind() == ErrorKind::IsADirectory => Ok(Self::Dir(path.to_path_buf())),
			Err(e) => Err(e.into()),
		}
	}

	pub fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
		match self {
			Self::Zip(zip) => match zip.borrow_mut().by_name(name) {
				Ok(mut file) => {
					let mut buf = Vec::new();
					file.read_to_end(&mut buf)?;
					Ok(Some(buf))
				}
				Err(ZipError::FileNotFound) => Ok(None),
				Err(e) => Err(e.into()),
			},
			Self::Dir(path) => match File::open(path.join(name)) {
				Ok(mut file) => {
					let mut buf = Vec::new();
					file.read_to_end(&mut buf)?;
					Ok(Some(buf))
				}
				Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
				Err(e) => Err(e.into()),
			},
		}
	}
}

pub fn load_image_from_paths(paths: &[MultiPath], name: &str) -> Result<Option<Image>> {
	let name = format!("{name}.png");
	let bytes = paths
		.iter()
		.filter_map(|x| {
			x.read(&name).unwrap_or_else(|e| {
				warn!(error=?e, "load_image_from_paths() returned error");
				None
			})
		})
		.next();
	let Some(bytes) = bytes else {
		return Ok(None);
	};

	// decode the image
	let cursor = Cursor::new(&bytes);
	let image = ImageReader::new(cursor)
		.with_guessed_format()
		.expect("cursor io never fails")
		.decode()?;

	// take the image and get the dimensions and raw bytes
	let rgba_image = image.to_rgba8();
	let width = rgba_image.width();
	let height = rgba_image.height();
	let raw_bytes = rgba_image.into_raw();

	// create a SharedPixelBuffer by cloning from the slice of pixels
	let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(raw_bytes.as_slice(), width, height);

	// create a slint::Image from the pixel buffer
	Ok(Some(slint::Image::from_rgba8(buffer)))
}

pub fn get_history_text(history: Option<&HistoryXml>, name: &str) -> SharedString {
	history
		.and_then(|history| {
			if name.is_empty() {
				None
			} else if let Some((list, software)) = name.split_once('/') {
				history.by_software(list, software)
			} else {
				history.by_system(name)
			}
		})
		.unwrap_or_default()
}

impl HistoryLoader {
	pub fn new(completed: impl Fn() + 'static + Send + Sync + Clone) -> Self {
		let worker_function = move |path, state: &State| {
			let result = HistoryXml::load(path, || state.is_cancelled()).transpose();
			completed();
			result
		};
		Self(CancelableWorker::new(worker_function))
	}

	pub fn load(&mut self, path: Option<impl Into<PathBuf>>) {
		self.0.cancel_tasks();
		if let Some(path) = path {
			self.0.add_task(path.into());
		}
	}

	pub fn take_result(&mut self) -> Option<Result<HistoryXml>> {
		self.0.get()
	}
}
