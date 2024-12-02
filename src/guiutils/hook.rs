use std::cell::RefCell;

use winit::window::WindowAttributes;

type WindowAttributeHookCallback = Box<dyn Fn(WindowAttributes) -> WindowAttributes + 'static>;
thread_local! {
	static WINDOW_ATTRIBUTE_HOOK_CALLBACK: RefCell<Option<WindowAttributeHookCallback>> = const { RefCell::new(None) }
}

/// creates a global attributes hook for setting up the Slint backend
pub fn create_window_attributes_hook(
	global_hook: impl Fn(WindowAttributes) -> WindowAttributes + 'static,
) -> Option<Box<dyn Fn(WindowAttributes) -> WindowAttributes>> {
	let hook = move |attrs| {
		// invoke the global hook
		let attrs = global_hook(attrs);

		WINDOW_ATTRIBUTE_HOOK_CALLBACK.with_borrow(|callback| {
			if let Some(callback) = callback {
				callback(attrs)
			} else {
				attrs
			}
		})
	};
	Some(Box::new(hook))
}

pub fn with_attributes_hook<T>(
	func: impl FnOnce() -> T,
	hook: impl Fn(WindowAttributes) -> WindowAttributes + 'static,
) -> T {
	// stow the callback
	WINDOW_ATTRIBUTE_HOOK_CALLBACK.set(Some(Box::new(hook)));

	// invoke the function
	let result = func();

	// clear out the hook
	let old_hook = WINDOW_ATTRIBUTE_HOOK_CALLBACK.take();
	assert!(old_hook.is_some(), "WINDOW_ATTRIBUTE_HOOK_CALLBACK was lost");

	// and return
	result
}
