//! Logic abstracting the differences between the various types of Slint back ends
#[cfg(feature = "slint-qt-backend")]
mod qt;
mod winit;

use std::rc::Rc;

use anyhow::Result;
use easy_ext::ext;
use i_slint_backend_winit::WinitWindowAccessor;
use slint::PhysicalPosition;
use slint::PhysicalSize;
use slint::Window;
use strum::EnumString;
use tracing::debug;
use tracing::warn;

use crate::backend::winit::SlintWindowExt;
use crate::backend::winit::WinitBackendRuntime;
use crate::backend::winit::WinitChildWindow;
use crate::backend::winit::WinitWindowExt;

pub use crate::backend::winit::WinitAccelerator;

#[cfg(feature = "slint-qt-backend")]
use crate::backend::qt::QtBackendRuntime;
#[cfg(feature = "slint-qt-backend")]
use crate::backend::qt::QtChildWindow;

#[derive(Debug, EnumString)]
pub enum SlintBackend {
	#[strum(ascii_case_insensitive)]
	Winit,

	#[cfg(feature = "slint-qt-backend")]
	#[strum(ascii_case_insensitive)]
	Qt,
}

#[derive(Clone)]
pub enum BackendRuntime {
	Winit(WinitBackendRuntime),

	#[cfg(feature = "slint-qt-backend")]
	Qt(QtBackendRuntime),
}

pub enum ChildWindow {
	Winit(Rc<WinitChildWindow>),

	#[cfg(feature = "slint-qt-backend")]
	Qt(QtChildWindow),
}

impl BackendRuntime {
	pub fn new(backend_type: SlintBackend) -> Result<Self> {
		// create an appropriate backends
		let (slint_backend, backend_runtime) = match backend_type {
			SlintBackend::Winit => {
				// create the Winit backend runtime
				let backend_runtime = WinitBackendRuntime::default();
				let slint_backend = backend_runtime.create_slint_backend()?;
				let backend_runtime = BackendRuntime::Winit(backend_runtime);
				(slint_backend, backend_runtime)
			}

			#[cfg(feature = "slint-qt-backend")]
			SlintBackend::Qt => {
				// create the Qt backend runtime
				let backend_runtime = QtBackendRuntime::default();
				let backend_runtime = BackendRuntime::Qt(backend_runtime);
				let slint_backend = QtBackendRuntime::create_slint_backend();
				(slint_backend, backend_runtime)
			}
		};

		// and specify the Slint backend
		slint::platform::set_platform(slint_backend)?;

		// and return our runtime
		Ok(backend_runtime)
	}

	pub async fn wait_for_window_ready(&self, window: &Window) -> Result<()> {
		match self {
			Self::Winit(backend) => backend.wait_for_window_ready(window).await,

			#[cfg(feature = "slint-qt-backend")]
			Self::Qt(backend) => backend.wait_for_window_ready(window).await,
		}
	}

	pub async fn create_child_window(&self, parent: &Window) -> Result<ChildWindow> {
		let child_window = match self {
			Self::Winit(backend) => ChildWindow::Winit(backend.create_child_window(parent).await?),

			#[cfg(feature = "slint-qt-backend")]
			Self::Qt(backend) => ChildWindow::Qt(backend.create_child_window(parent)?),
		};
		Ok(child_window)
	}

	pub fn install_muda_accelerator_handler(
		&self,
		window: &Window,
		callback: impl Fn(&WinitAccelerator) -> bool + 'static,
	) {
		match self {
			Self::Winit(backend) => backend.install_muda_accelerator_handler(window, callback),

			#[cfg(feature = "slint-qt-backend")]
			Self::Qt(_backend) => todo!(),
		}
	}

	pub fn with_modal_parent<R>(&self, window: &Window, callback: impl FnOnce() -> R) -> R {
		match self {
			Self::Winit(backend) => backend.with_modal_parent(window, callback),

			#[cfg(feature = "slint-qt-backend")]
			Self::Qt(_) => callback(),
		}
	}
}

impl ChildWindow {
	pub fn set_active(&self, active: bool) {
		match self {
			Self::Winit(child_window) => child_window.set_active(active),

			#[cfg(feature = "slint-qt-backend")]
			Self::Qt(child_window) => child_window.set_active(active),
		}
	}

	pub fn update_bounds(&self, container: &Window, top: f32) {
		let position = PhysicalPosition {
			x: 0,
			y: (top * container.scale_factor()) as i32,
		};
		let size = container.size();
		let size = PhysicalSize::new(size.width, size.height - (position.y as u32));
		debug!(position=?position, size=?size, "ChildWindow::update_bounds()");

		let position =
			dpi::PhysicalPosition::<u32>::new(position.x.try_into().unwrap(), position.y.try_into().unwrap());
		let size = dpi::PhysicalSize::<u32>::new(size.width, size.height);

		match self {
			Self::Winit(child_window) => child_window.set_position_and_size(position, size),

			#[cfg(feature = "slint-qt-backend")]
			Self::Qt(child_window) => child_window.set_position_and_size(position, size),
		}
	}

	pub fn text(&self) -> String {
		match self {
			Self::Winit(child_window) => child_window.text(),

			#[cfg(feature = "slint-qt-backend")]
			Self::Qt(child_window) => child_window.text(),
		}
	}
}

impl Default for SlintBackend {
	fn default() -> Self {
		#[cfg(feature = "slint-qt-backend")]
		let result = Self::Qt;

		#[cfg(not(feature = "slint-qt-backend"))]
		let result = Self::Winit;

		result
	}
}

#[ext(WindowExt)]
pub impl Window {
	fn fullscreen_display(&self) -> Option<String> {
		self.with_winit_window(|window| window.fullscreen_display()).flatten()
	}

	fn set_fullscreen_with_display(&self, fullscreen: bool, display: Option<&str>) {
		let result = if fullscreen && let Some(display) = display {
			SlintWindowExt::set_fullscreen_with_display(self, display)
		} else {
			Ok(false)
		};

		if result.is_err() {
			warn!(result=?result, "Failed to set fullscreen");
		}
		if result.is_ok_and(|x| x) {
			self.set_fullscreen(fullscreen);
		}
	}
}
