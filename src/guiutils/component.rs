//! Wrappers for Slint `ComponentHandle` and `Weak<ComponentHandle>`, because its hard to work with them generically
use std::rc::Rc;

use slint::ComponentHandle;
use slint::Weak;
use slint::Window;

pub struct ComponentWrap(Rc<dyn ComponentWrapTrait + 'static>);

#[derive(Clone)]
pub struct WeakComponentWrap(Rc<dyn WeakComponentWrapTrait + 'static>);

struct Wrap<T>(T);

trait ComponentWrapTrait {
	fn window(&self) -> &Window;
	fn as_weak(&self) -> WeakComponentWrap;
}

trait WeakComponentWrapTrait {
	fn upgrade(&self) -> Option<ComponentWrap>;
}

impl ComponentWrap {
	pub fn window(&self) -> &Window {
		self.0.window()
	}

	pub fn as_weak(&self) -> WeakComponentWrap {
		self.0.as_weak()
	}

	pub fn clone_strong(&self) -> Self {
		Self(self.0.clone())
	}
}

impl<H> From<H> for ComponentWrap
where
	H: ComponentHandle + 'static,
{
	fn from(value: H) -> Self {
		let value = Wrap(value);
		let value = Rc::new(value);
		let value = value as Rc<dyn ComponentWrapTrait + 'static>;
		Self(value)
	}
}

impl WeakComponentWrap {
	pub fn upgrade(&self) -> Option<ComponentWrap> {
		self.0.upgrade()
	}

	pub fn unwrap(&self) -> ComponentWrap {
		self.upgrade().unwrap()
	}
}

impl<H> From<&H> for WeakComponentWrap
where
	H: ComponentHandle + 'static,
{
	fn from(value: &H) -> Self {
		value.as_weak().into()
	}
}

impl<H> From<Weak<H>> for WeakComponentWrap
where
	H: ComponentHandle + 'static,
{
	fn from(value: Weak<H>) -> Self {
		let weak = Wrap(value);
		let weak = Rc::new(weak);
		let weak = weak as Rc<dyn WeakComponentWrapTrait + 'static>;
		Self(weak)
	}
}

impl<H> ComponentWrapTrait for Wrap<H>
where
	H: ComponentHandle + 'static,
{
	fn window(&self) -> &Window {
		self.0.window()
	}

	fn as_weak(&self) -> WeakComponentWrap {
		self.0.as_weak().into()
	}
}

impl<H> WeakComponentWrapTrait for Wrap<Weak<H>>
where
	H: ComponentHandle + 'static,
{
	fn upgrade(&self) -> Option<ComponentWrap> {
		let component_handle = self.0.upgrade()?;
		Some(component_handle.into())
	}
}
