use easy_ext::ext;
use tokio::sync::mpsc::Sender;

pub mod configure;
pub mod devimages;
pub mod file;
pub mod image;
pub mod input;
pub mod messagebox;
pub mod namecollection;
pub mod paths;
pub mod seqpoll;
pub mod socket;

#[ext(SenderExt)]
impl<T> Sender<T> {
	/// Send a value, and drop if the channel is full
	pub fn signal(&self, value: T) {
		let _ = self.try_send(value);
	}
}
