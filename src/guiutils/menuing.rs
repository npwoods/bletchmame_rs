//! Helpers for Menu handling; which Slint does not handle yet
use muda::accelerator::Accelerator;
use muda::accelerator::Code;
use muda::accelerator::Modifiers;

/// Helper function to declare accelerators
pub fn accel(text: &str) -> Option<Accelerator> {
	fn strip_modifier<'a>(
		text: &'a str,
		mods: Option<Modifiers>,
		prefix: &str,
		prefix_mod: Modifiers,
	) -> (&'a str, Option<Modifiers>) {
		text.strip_prefix(prefix)
			.map(|text| (text, Some(mods.unwrap_or_default() | prefix_mod)))
			.unwrap_or((text, mods))
	}

	let mods = None;
	let (text, mods) = strip_modifier(text, mods, "Ctrl+", Modifiers::CONTROL);
	let (text, mods) = strip_modifier(text, mods, "Shift+", Modifiers::SHIFT);
	let (text, mods) = strip_modifier(text, mods, "Alt+", Modifiers::ALT);

	let key = match text {
		"X" => Code::KeyX,
		"F7" => Code::F7,
		"F8" => Code::F8,
		"F9" => Code::F9,
		"F10" => Code::F10,
		"F11" => Code::F11,
		"F12" => Code::F12,
		"Pause" => Code::Pause,
		"ScrLk" => Code::ScrollLock,
		x => panic!("Unknown accelerator {x}"),
	};
	Some(Accelerator::new(mods, key))
}

#[cfg(test)]
mod test {
	use muda::accelerator::Accelerator;
	use muda::accelerator::Code;
	use muda::accelerator::Modifiers;
	use test_case::test_case;

	#[test_case(0, "X", Accelerator::new(None, Code::KeyX))]
	#[test_case(1, "Ctrl+X", Accelerator::new(Some(Modifiers::CONTROL), Code::KeyX))]
	#[test_case(2, "Shift+X", Accelerator::new(Some(Modifiers::SHIFT), Code::KeyX))]
	#[test_case(3, "Ctrl+Alt+X", Accelerator::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyX))]
	pub fn accel(_index: usize, text: &str, expected: Accelerator) {
		let actual = super::accel(text);
		assert_eq!(Some(expected), actual);
	}
}
