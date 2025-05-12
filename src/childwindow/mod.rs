#[cfg(feature = "slint-qt-backend")]
mod qt;
mod winit;

use anyhow::Result;
use slint::PhysicalPosition;
use slint::PhysicalSize;
use slint::Window;
use tracing::debug;

use crate::childwindow::winit::WinitChildWindow;

#[cfg(feature = "slint-qt-backend")]
use crate::childwindow::qt::QtChildWindow;

trait ChildWindowImpl {
	fn set_active(&self, active: bool);
	fn update(&self, position: dpi::PhysicalPosition<u32>, size: dpi::PhysicalSize<u32>);
	fn text(&self) -> String;

	/// Hackish (and platform specific) method to "ensure" focus
	fn ensure_child_focus(&self, container: &Window);
}

#[cfg(not(feature = "slint-qt-backend"))]
pub struct ChildWindow(WinitChildWindow);
#[cfg(feature = "slint-qt-backend")]
pub struct ChildWindow(Box<dyn ChildWindowImpl>);

impl ChildWindow {
	#[cfg(not(feature = "slint-qt-backend"))]
	pub fn new(parent: &Window) -> Result<Self> {
		let result = WinitChildWindow::new(parent)?;
		Ok(Self(result))
	}

	#[cfg(feature = "slint-qt-backend")]
	pub fn new(parent: &Window) -> Result<Self> {
		use i_slint_backend_winit::WinitWindowAccessor;
		let result = if parent.with_winit_window(|_| ()).is_some() {
			Box::new(WinitChildWindow::new(parent)?) as Box<_>
		} else {
			Box::new(QtChildWindow::new(parent)?) as Box<_>
		};
		Ok(Self(result))
	}

	pub fn set_active(&self, active: bool) {
		self.0.set_active(active);
	}

	pub fn update(&self, container: &Window, top: f32) {
		let position = PhysicalPosition {
			x: 0,
			y: (top * container.scale_factor()) as i32,
		};
		let size = container.size();
		let size = PhysicalSize::new(size.width, size.height - (position.y as u32));
		debug!(position=?position, size=?size, "ChildWindow::update()");

		let position =
			dpi::PhysicalPosition::<u32>::new(position.x.try_into().unwrap(), position.y.try_into().unwrap());
		let size = dpi::PhysicalSize::<u32>::new(size.width, size.height);
		self.0.update(position, size);
	}

	pub fn text(&self) -> String {
		self.0.text()
	}

	pub fn ensure_child_focus(&self, container: &Window) {
		self.0.ensure_child_focus(container);
	}
}
