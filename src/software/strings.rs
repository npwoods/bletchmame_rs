use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct StringDispenser(Mutex<HashSet<Arc<str>>>);

impl StringDispenser {
	pub fn get(&self, s: &str) -> Arc<str> {
		let mut guard = self.0.lock().unwrap();
		guard.get(s).cloned().unwrap_or_else(|| {
			let result = Arc::<str>::from(s);
			guard.insert(result.clone());
			result
		})
	}
}
