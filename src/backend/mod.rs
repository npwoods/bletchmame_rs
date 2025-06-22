//! Logic abstracting the differences between the various types of Slint back ends
#[cfg(feature = "slint-qt-backend")]
mod qt;
mod winit;

use std::rc::Rc;

use anyhow::Result;
use slint::Window;
use strum::EnumString;

use crate::backend::winit::WinitBackendRuntime;
use crate::backend::winit::WinitChildWindow;

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

	pub async fn create_child_window(&self, parent: &Window) -> Result<ChildWindow> {
		let child_window = match self {
			Self::Winit(backend) => ChildWindow::Winit(backend.create_child_window(parent).await?),

			#[cfg(feature = "slint-qt-backend")]
			Self::Qt(backend) => ChildWindow::Qt(backend.create_child_window(parent)?),
		};
		Ok(child_window)
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
