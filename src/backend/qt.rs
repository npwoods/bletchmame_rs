use std::cell::Cell;
use std::rc::Rc;

use anyhow::Result;
use i_slint_backend_qt::QtWidgetAccessor;

use crate::backend::qt::qtwidget::QtWidget;

#[derive(Clone, Debug, Default)]
pub struct QtBackendRuntime {}

pub struct QtChildWindow {
	qt_widget: QtWidget,
	geometry: Cell<(i32, i32, i32, i32)>,
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("cannot create child window")]
	CannotCreateChildWindow,
}

impl QtBackendRuntime {
	pub fn create_slint_backend() -> Box<dyn slint::platform::Platform> {
		let slint_backend = i_slint_backend_qt::Backend::new();
		Box::new(slint_backend) as Box<_>
	}

	pub fn create_child_window(&self, parent: &slint::Window) -> Result<QtChildWindow> {
		let parent = parent.qt_widget_ptr().ok_or(ThisError::CannotCreateChildWindow)?;
		let qt_widget = QtWidget::new(parent);
		let geometry = Cell::new((0, 0, 100, 100));
		let result = QtChildWindow { qt_widget, geometry };
		result.internal_update(Some(false));
		Ok(result)
	}

	pub fn install_scroll_lock_handler(&self, _window: &slint::Window, _callback: Rc<dyn Fn() + 'static>) {
		// not yet implemented
	}
}

impl QtChildWindow {
	pub fn set_active(&self, active: bool) {
		if active != self.qt_widget.is_visible() {
			self.internal_update(Some(active));
		}
	}

	pub fn set_position_and_size(&self, position: dpi::PhysicalPosition<u32>, size: dpi::PhysicalSize<u32>) {
		let geometry = (
			position.x.try_into().unwrap(),
			position.y.try_into().unwrap(),
			size.width.try_into().unwrap(),
			size.width.try_into().unwrap(),
		);
		self.geometry.set(geometry);
		self.internal_update(None);
	}

	pub fn text(&self) -> String {
		self.qt_widget.win_id().to_string()
	}

	fn internal_update(&self, active: Option<bool>) {
		if let Some(active) = active {
			self.qt_widget.set_visible(active);
		}

		let active = active.unwrap_or_else(|| self.qt_widget.is_visible());
		let (x, y, w, h) = if active {
			self.geometry.get()
		} else {
			(-200, -200, 100, 100)
		};
		self.qt_widget.set_geometry(x, y, w, h);
	}
}

mod qtwidget {
	use std::ptr::NonNull;

	use cpp::cpp;
	use cpp::cpp_class;

	cpp! {{
		#include <QtWidgets/QtWidgets>
		#include <memory>
	}}

	cpp_class!(pub(crate) unsafe struct QWidgetPtr as "std::unique_ptr<QWidget>");

	pub struct QtWidget(QWidgetPtr);

	impl QtWidget {
		pub fn new(parent: NonNull<()>) -> Self {
			let qt_widget = cpp!(unsafe [parent as "QWidget *"] -> QWidgetPtr as "std::unique_ptr<QWidget>" {
				return std::make_unique<QWidget>(parent);
			});
			Self(qt_widget)
		}

		pub fn set_visible(&self, visible: bool) {
			let qt_widget = &self.0;
			cpp!(unsafe [qt_widget as "std::unique_ptr<QWidget> *", visible as "bool"] {
				(*qt_widget)->setVisible(visible);
			});
		}

		pub fn is_visible(&self) -> bool {
			let qt_widget = &self.0;
			cpp!(unsafe [qt_widget as "std::unique_ptr<QWidget> *" ] -> bool as "bool" {
				return (*qt_widget)->isVisible();
			})
		}

		pub fn set_geometry(&self, x: i32, y: i32, w: i32, h: i32) {
			let qt_widget = &self.0;
			cpp!(unsafe [qt_widget as "std::unique_ptr<QWidget> *", x as "int", y as "int", w as "int", h as "int" ] {
				(*qt_widget)->setGeometry(x, y, w, h);
			});
		}

		pub fn win_id(&self) -> usize {
			let qt_widget = &self.0;
			cpp!(unsafe [qt_widget as "std::unique_ptr<QWidget> *" ] -> usize as "WId" {
				return (*qt_widget)->winId();
			})
		}
	}
}
