use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Result;
use i_slint_backend_winit::CustomApplicationHandler;
use i_slint_backend_winit::WinitWindowAccessor;
use i_slint_backend_winit::WinitWindowEventResult;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use tokio::sync::oneshot;
use tokio::sync::oneshot::Sender;
use tracing::debug;
use tracing::info_span;
use winit::event::ElementState;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::Key;
use winit::keyboard::NamedKey;
use winit::window::Window;
use winit::window::WindowAttributes;
use winit::window::WindowId;

#[derive(Clone, Default)]
pub struct WinitBackendRuntime(Rc<RefCell<WinitBackendRuntimeInner>>);

#[derive(Default)]
struct WinitBackendRuntimeInner {
	pending: Vec<WinitPendingChildWindow>,
	live: Vec<Rc<WinitChildWindow>>,
	scroll_lock_handlers: Vec<(WindowId, Rc<dyn Fn()>)>,
}

#[derive(Debug)]
pub struct WinitChildWindow {
	window: Window,
	parent_window_id: WindowId,
}

#[derive(Debug)]
struct WinitPendingChildWindow {
	parent_window_id: WindowId,
	window_attributes: WindowAttributes,
	sender: Sender<Result<WinitChildWindow>>,
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("unknown raw handle type")]
	UnknownRawHandleType,
}

#[derive(Debug)]
enum FindResult {
	None,
	Parent(Rc<WinitChildWindow>),
	Child(Rc<WinitChildWindow>),
}

impl WinitBackendRuntime {
	pub fn create_slint_backend(&self) -> Result<Box<dyn slint::platform::Platform>> {
		let slint_backend = i_slint_backend_winit::Backend::builder()
			.with_custom_application_handler(self.clone())
			.build()?;
		Ok(Box::new(slint_backend) as Box<_>)
	}

	pub async fn create_child_window(&self, parent: &slint::Window) -> Result<Rc<WinitChildWindow>> {
		// prepare the window attributes
		let raw_window_handle = parent.window_handle().window_handle()?.as_raw();
		let size = parent.with_winit_window(|parent| parent.inner_size()).unwrap();
		let window_attributes = unsafe {
			WindowAttributes::default()
				.with_title("MAME Child Window")
				.with_visible(false)
				.with_inner_size(size)
				.with_decorations(false)
				.with_parent_window(Some(raw_window_handle))
		};

		// we're going to need to wait for the window to be created in
		// an `ActiveEventLoop`; prepare to receive it
		let (sender, receiver) = oneshot::channel();

		// create and push the pending window
		let parent_window_id = parent.with_winit_window(|window| window.id()).unwrap();
		let pending_child_window = WinitPendingChildWindow {
			parent_window_id,
			window_attributes,
			sender,
		};
		self.0.borrow_mut().pending.push(pending_child_window);

		// and await the result
		let child_window = receiver.await??;

		// wrap it up in an Rc and add it to "live"
		let result = Rc::new(child_window);
		self.0.borrow_mut().live.push(result.clone());

		// and return the result
		Ok(result)
	}

	pub fn install_scroll_lock_handler(&self, window: &slint::Window, callback: Rc<dyn Fn() + 'static>) {
		let window_id = window
			.with_winit_window(|window| window.id())
			.expect("could not get WindowId");
		self.0.borrow_mut().scroll_lock_handlers.push((window_id, callback));
	}

	fn create_pending_child_windows(&self, event_loop: &ActiveEventLoop) {
		let mut state = self.0.borrow_mut();

		// we need to create child windows
		for pending in state.pending.drain(..) {
			let result = WinitChildWindow::new(pending.parent_window_id, pending.window_attributes, event_loop);
			let _ = pending.sender.send(result);
		}
	}

	fn find_child_window(&self, window_id: &WindowId) -> FindResult {
		self.0
			.borrow()
			.live
			.iter()
			.filter_map(|x| {
				if &x.parent_window_id == window_id {
					Some(FindResult::Parent(x.clone()))
				} else if x.window.id() == *window_id {
					Some(FindResult::Child(x.clone()))
				} else {
					None
				}
			})
			.next()
			.unwrap_or(FindResult::None)
	}
}

impl CustomApplicationHandler for WinitBackendRuntime {
	fn window_event(
		&mut self,
		event_loop: &ActiveEventLoop,
		window_id: WindowId,
		_winit_window: Option<&Window>,
		_slint_window: Option<&slint::Window>,
		event: &WindowEvent,
	) -> WinitWindowEventResult {
		// tracing
		let span = info_span!("window_event");
		let _guard = span.enter();
		debug!(event=?event, "window_event");

		// take this opportunity to create pending children, regardless of what is going on
		self.create_pending_child_windows(event_loop);

		match event {
			WindowEvent::Focused(true) => match self.find_child_window(&window_id) {
				FindResult::Parent(child_window) => {
					if child_window.is_active() {
						child_window.fix_focus();
					}
				}
				FindResult::Child(child_window) => {
					if !child_window.is_active() {
						child_window.fix_focus();
					}
				}
				FindResult::None => {}
			},

			WindowEvent::KeyboardInput { event, .. } => {
				if event.logical_key == Key::Named(NamedKey::ScrollLock) && event.state == ElementState::Released {
					let callback = {
						let state = self.0.borrow();
						let window_id = state
							.live
							.iter()
							.find(|x| x.window.id() == window_id)
							.map(|x| &x.parent_window_id)
							.unwrap_or(&window_id);
						state
							.scroll_lock_handlers
							.iter()
							.find(|(this_window_id, _)| window_id == this_window_id)
							.map(|(_, callback)| callback)
							.cloned()
					};
					if let Some(callback) = callback.as_deref() {
						callback();
					}
				}
			}

			WindowEvent::Destroyed => {
				let mut state = self.0.borrow_mut();
				state
					.live
					.retain(|x| x.parent_window_id != window_id && x.window.id() != window_id);
				state.scroll_lock_handlers.retain(|(x, _)| *x != window_id);
			}
			_ => {}
		}
		WinitWindowEventResult::Propagate
	}

	fn resumed(&mut self, event_loop: &ActiveEventLoop) -> WinitWindowEventResult {
		self.create_pending_child_windows(event_loop);
		WinitWindowEventResult::Propagate
	}
}

impl WinitChildWindow {
	pub fn new(
		parent_window_id: WindowId,
		window_attributes: WindowAttributes,
		event_loop: &ActiveEventLoop,
	) -> Result<Self> {
		// create the window
		let window = event_loop.create_window(window_attributes)?;

		// prepare the result
		let result = Self {
			parent_window_id,
			window,
		};

		// sanity check it
		result.try_text()?;

		// and return!
		Ok(result)
	}

	pub fn set_active(&self, active: bool) {
		self.window.set_visible(active);
	}

	pub fn set_position_and_size(&self, position: dpi::PhysicalPosition<u32>, size: dpi::PhysicalSize<u32>) {
		self.window.set_outer_position(position);
		let _ = self.window.request_inner_size(size);
	}

	pub fn text(&self) -> String {
		self.try_text().unwrap()
	}

	pub fn is_active(&self) -> bool {
		self.window.is_visible().unwrap_or_default()
	}

	pub fn fix_focus(&self) {
		#[cfg(target_family = "windows")]
		if let RawWindowHandle::Win32(win32_window_handle) = self.window.window_handle().unwrap().as_raw() {
			use tracing::debug;
			use windows::Win32::Foundation::HWND;
			use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
			use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
			use windows::Win32::UI::WindowsAndMessaging::GetParent;

			let active = self.is_active();
			let child_hwnd = HWND(win32_window_handle.hwnd.get() as *mut std::ffi::c_void);
			let parent_hwnd = unsafe { GetParent(child_hwnd) };
			let focus_hwnd = unsafe { GetFocus() };

			let set_focus_hwnd = if active {
				(Ok(focus_hwnd) == parent_hwnd).then_some(child_hwnd)
			} else {
				(focus_hwnd == child_hwnd).then_some(parent_hwnd.clone().ok()).flatten()
			};

			debug!(parent_hwnd=?parent_hwnd, child_hwnd=?child_hwnd, focus_hwnd=?focus_hwnd, active=?active, set_focus_hwnd=?set_focus_hwnd, "WinitChildWindow::fix_focus()");

			if let Some(set_focus_hwnd) = set_focus_hwnd {
				let _ = unsafe { SetFocus(Some(set_focus_hwnd)) };
			}
		}
	}

	fn try_text(&self) -> Result<String> {
		let raw_window_handle = self.window.window_handle().unwrap().as_raw();
		match raw_window_handle {
			#[cfg(target_family = "windows")]
			RawWindowHandle::Win32(win32_window_handle) => Ok(win32_window_handle.hwnd.to_string()),

			#[cfg(target_family = "unix")]
			RawWindowHandle::Xlib(xlib_window_handle) => Ok(xlib_window_handle.window.to_string()),

			_ => Err(ThisError::UnknownRawHandleType.into()),
		}
	}
}
