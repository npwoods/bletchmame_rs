use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::spawn_local;

pub struct SelectionManager {
	getter: Box<dyn Fn() -> i32 + 'static>,
	setter: Rc<dyn Fn(i32) + 'static>,
	index_to_select: Cell<Option<i32>>,
}

impl SelectionManager {
	pub fn new<C>(component: &C, getter: impl Fn(&C) -> i32 + 'static, setter: impl Fn(&C, i32) + 'static) -> Self
	where
		C: ComponentHandle + 'static,
	{
		let component_weak = component.as_weak();
		let getter = move || getter(&component_weak.unwrap());
		let component_weak = component.as_weak();
		let setter = move |index| setter(&component_weak.unwrap(), index);
		Self::new_internal(getter, setter)
	}

	pub fn new_internal(getter: impl Fn() -> i32 + 'static, setter: impl Fn(i32) + 'static) -> Self {
		let getter = Box::from(getter) as Box<dyn Fn() -> i32>;
		let setter = Rc::from(setter) as Rc<dyn Fn(i32)>;
		Self {
			getter,
			setter,
			index_to_select: Cell::new(None),
		}
	}

	pub fn selected_index(&self) -> Option<usize> {
		(self.getter)().try_into().ok()
	}

	pub fn set_selected_index(&self, index: Option<usize>) {
		let index = index.map(|x| x.try_into().unwrap()).unwrap_or(-1);
		self.index_to_select.set(Some(index));
	}

	pub fn model_accessed(&self) {
		if let Some(index) = self.index_to_select.take() {
			let setter = self.setter.clone();
			let fut = async move {
				setter(index);
			};
			spawn_local(fut).unwrap();
		}
	}
}
