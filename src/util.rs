use easy_ext::ext;
use slint::ModelRc;
use slint::VecModel;

#[ext(IteratorExt)]
pub impl<I, T> I
where
	Self: Iterator<Item = T>,
{
	fn collect_model_rc(self) -> ModelRc<T>
	where
		T: Clone + 'static,
	{
		let vec_model = VecModel::from_iter(self);
		ModelRc::new(vec_model)
	}
}
