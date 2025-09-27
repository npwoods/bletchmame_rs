use std::borrow::Cow;
use std::cell::RefCell;
use std::fs::File;
use std::io::Cursor;
use std::io::ErrorKind;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use image::ImageReader;
use slint::Image;
use slint::Rgba8Pixel;
use slint::SharedPixelBuffer;
use slint::SharedString;
use smol_str::SmolStr;
use tracing::info;
use tracing::warn;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::history_xml::HistoryXml;
use crate::prefs::PrefsItem;
use crate::prefs::PrefsMachineItem;
use crate::prefs::PrefsSoftwareItem;

pub struct SnapView {
	callback: Box<dyn Fn(SnapViewCallbackInfo)>,
	state: RefCell<SnapViewState>,
}

#[derive(Default, Debug)]
struct SnapViewState {
	snapshot_paths: Vec<MultiPath>,
	history: Option<HistoryXml>,
	current_snap: Option<SmolStr>,
}

#[derive(Default, Debug)]
pub struct SnapViewCallbackInfo {
	pub snap: Option<Option<Image>>,
	pub history_text: Option<SharedString>,
}

#[derive(Debug)]
enum MultiPath {
	Zip(RefCell<ZipArchive<File>>),
	Dir(PathBuf),
}

impl SnapView {
	pub fn new(callback: impl Fn(SnapViewCallbackInfo) + 'static) -> Self {
		let callback = Box::new(callback) as Box<_>;
		let state = RefCell::new(SnapViewState::default());
		Self { callback, state }
	}

	pub fn set_paths(
		&self,
		snapshot_paths: Option<&[impl AsRef<Path>]>,
		history_file_path: Option<Option<impl AsRef<Path>>>,
	) {
		if let Some(snapshot_paths) = snapshot_paths {
			let new_snapshot_paths = snapshot_paths
				.iter()
				.filter_map(|path| match MultiPath::new(path) {
					Ok(multi_path) => Some(multi_path),
					Err(e) => {
						warn!(error=?e, path=?path.as_ref(), "MultiPath::new() returned error");
						None
					}
				})
				.collect::<Vec<_>>();
			self.state.borrow_mut().snapshot_paths = new_snapshot_paths;
		}

		if let Some(history_file_path) = history_file_path {
			let history_file_path = history_file_path.as_ref().map(|p| p.as_ref());
			let history = history_file_path.map(HistoryXml::load).transpose().unwrap_or_else(|e| {
				warn!(error=?e, path=?history_file_path, "HistoryXml::load() returned error");
				None
			});
			self.state.borrow_mut().history = history;
		}
	}

	pub fn set_current_item(&self, item: Option<&PrefsItem>) {
		// determine the "name" of the current item (e.g. - "pacman" or "nes/zelda")
		let name = match item {
			Some(PrefsItem::Machine(PrefsMachineItem { machine_name, .. })) => {
				Some(Cow::Borrowed(machine_name.as_str()))
			}
			Some(PrefsItem::Software(PrefsSoftwareItem {
				software_list,
				software,
				..
			})) => Some(Cow::Owned(format!("{software_list}/{software}"))),
			_ => None,
		};
		let name = name.as_deref();

		let snap = {
			let mut state = self.state.borrow_mut();
			(name != state.current_snap.as_deref()).then(|| {
				let image = name.and_then(|name| {
					load_image_from_paths(&state.snapshot_paths, name).unwrap_or_else(|e| {
						warn!(error=?e, name=?name, "load_image_from_paths() returned error");
						None
					})
				});
				state.current_snap = image.is_some().then(|| name.unwrap().into());
				image
			})
		};

		let history_text = self.state.borrow().history.as_ref().and_then(|history| match item {
			Some(PrefsItem::Machine(PrefsMachineItem { machine_name, .. })) => history.by_system(machine_name.as_str()),
			Some(PrefsItem::Software(PrefsSoftwareItem {
				software_list,
				software,
				..
			})) => history.by_software(software_list, software),
			_ => None,
		});
		let history_text = Some(history_text.unwrap_or_default());

		if snap.is_some() || history_text.is_some() {
			info!(snap=?snap.as_ref().map(|x| x.as_ref().map(|_| "...")), history_text=?history_text.as_ref().map(|_| "..."), "SnapView::set_current_item(): invoking callback");
			let svci = SnapViewCallbackInfo { snap, history_text };
			(self.callback)(svci)
		}
	}
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

fn load_image_from_paths(paths: &[MultiPath], name: &str) -> Result<Option<Image>> {
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
