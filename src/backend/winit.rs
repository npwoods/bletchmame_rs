use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Result;
use easy_ext::ext;
use i_slint_backend_winit::CustomApplicationHandler;
use i_slint_backend_winit::EventResult;
use i_slint_backend_winit::WinitWindowAccessor;
use raw_window_handle::HasWindowHandle;
use raw_window_handle::RawWindowHandle;
use tokio::sync::oneshot;
use tokio::sync::oneshot::Sender;
use tracing::debug;
use tracing::info_span;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::window::Fullscreen;
use winit::window::Window;
use winit::window::WindowAttributes;
use winit::window::WindowId;

use crate::platform::WindowAttributesExt;

#[derive(Clone, Default)]
pub struct WinitBackendRuntime(Rc<RefCell<WinitBackendRuntimeInner>>);

#[derive(Default)]
struct WinitBackendRuntimeInner {
	pending: Vec<WinitPendingChildWindow>,
	live: Vec<Rc<WinitChildWindow>>,
	modal_parent_raw_handle: Option<RawWindowHandle>,
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
	#[error("cannot find display \"{0}\"")]
	CannotFindDisplay(String),
}

#[derive(Debug, PartialEq, Eq)]
enum FindResultType {
	Parent,
	Child,
}

impl WinitBackendRuntime {
	pub fn create_slint_backend(&self) -> Result<Box<dyn slint::platform::Platform>> {
		let self_clone = self.clone();
		let slint_backend = i_slint_backend_winit::Backend::builder()
			.with_custom_application_handler(Box::new(self.clone()))
			.with_window_attributes_hook(move |attr| {
				// this is necessary to make the menu bar visible in full screen mode (as per https://github.com/slint-ui/slint/issues/8793)
				let attr = attr.with_transparent(false);

				// specify an owner if possible
				if let Some(modal_parent_raw_handle) = self_clone.0.borrow().modal_parent_raw_handle.as_ref() {
					attr.with_owner_window_handle(modal_parent_raw_handle)
				} else {
					attr
				}
			})
			.build()?;
		Ok(Box::new(slint_backend) as Box<_>)
	}

	pub async fn wait_for_window_ready(&self, window: &slint::Window) -> Result<()> {
		let _ = window.winit_window().await?;
		Ok(())
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

		// set inactive
		child_window.do_set_active(false);

		// wrap it up in an Rc and add it to "live"
		let result = Rc::new(child_window);
		self.0.borrow_mut().live.push(result.clone());

		// and return the result
		Ok(result)
	}

	pub fn with_modal_parent<R>(&self, window: &slint::Window, callback: impl FnOnce() -> R) -> R {
		let modal_parent_raw_handle = window
			.with_winit_window(|window| window.window_handle().ok().map(|x| x.as_raw()))
			.flatten();
		self.0.borrow_mut().modal_parent_raw_handle = modal_parent_raw_handle;
		let result = callback();
		self.0.borrow_mut().modal_parent_raw_handle = None;
		result
	}

	fn create_pending_child_windows(&self, event_loop: &ActiveEventLoop) {
		let mut state = self.0.borrow_mut();

		// we need to create child windows
		for pending in state.pending.drain(..) {
			let result = WinitChildWindow::new(pending.parent_window_id, pending.window_attributes, event_loop);
			let _ = pending.sender.send(result);
		}
	}

	fn with_child_window<R>(
		&self,
		window_id: &WindowId,
		callback: impl FnOnce(&WinitChildWindow, FindResultType) -> R,
	) -> Option<R> {
		self.0
			.borrow()
			.live
			.iter()
			.map(Rc::as_ref)
			.filter_map(|child_window| {
				if &child_window.parent_window_id == window_id {
					Some((child_window, FindResultType::Parent))
				} else if child_window.window.id() == *window_id {
					Some((child_window, FindResultType::Child))
				} else {
					None
				}
			})
			.next()
			.map(|(child_window, find_result_type)| callback(child_window, find_result_type))
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
	) -> EventResult {
		// tracing
		let span = info_span!("window_event");
		let _guard = span.enter();
		debug!(?event, ?window_id, "window_event");

		// take this opportunity to create pending children, regardless of what is going on
		self.create_pending_child_windows(event_loop);

		match event {
			WindowEvent::Focused(true) => {
				self.with_child_window(&window_id, |child_window, find_result_type| {
					let expected_child_window_active = match find_result_type {
						FindResultType::Parent => false,
						FindResultType::Child => true,
					};
					if expected_child_window_active != child_window.is_active() {
						child_window.fix_focus();
					}
				});
				EventResult::Propagate
			}

			WindowEvent::KeyboardInput { .. } => self
				.with_child_window(&window_id, |child_window, find_result_type| {
					(find_result_type == FindResultType::Child)
						.then_some(EventResult::Retarget(child_window.parent_window_id))
				})
				.flatten()
				.unwrap_or(EventResult::Propagate),

			WindowEvent::Destroyed => {
				let mut state = self.0.borrow_mut();
				state
					.live
					.retain(|x| x.parent_window_id != window_id && x.window.id() != window_id);
				EventResult::Propagate
			}
			_ => EventResult::Propagate,
		}
	}

	fn resumed(&mut self, event_loop: &ActiveEventLoop) -> EventResult {
		self.create_pending_child_windows(event_loop);
		EventResult::Propagate
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
		if active != self.is_active() {
			self.do_set_active(active);
		}
	}

	fn do_set_active(&self, active: bool) {
		self.window.set_visible(active);

		#[cfg(target_family = "windows")]
		winit::platform::windows::WindowExtWindows::set_enable(&self.window, active);

		self.fix_focus();
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

#[ext(WinitWindowExt)]
pub impl Window {
	fn fullscreen_display(&self) -> Option<String> {
		let monitor = match self.fullscreen() {
			None => None,
			Some(Fullscreen::Exclusive(video_mode)) => Some(video_mode.monitor()),
			Some(Fullscreen::Borderless(monitor)) => monitor,
		};
		monitor.as_ref().and_then(MonitorHandle::name)
	}
}

#[ext(SlintWindowExt)]
pub impl slint::Window {
	fn set_fullscreen_with_display(&self, display: &str) -> Result<bool> {
		self.with_winit_window(|window| {
			let monitor = window
				.available_monitors()
				.find(|m| m.name().as_deref() == Some(display))
				.ok_or_else(|| ThisError::CannotFindDisplay(display.into()))?;
			let fullscreen = Some(Fullscreen::Borderless(Some(monitor)));
			window.set_fullscreen(fullscreen);
			Ok(true)
		})
		.unwrap_or(Ok(false))
	}
}
