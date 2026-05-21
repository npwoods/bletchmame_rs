use std::borrow::Cow;

use itertools::Itertools;

pub fn markdown_from_history_text(text: &str) -> String {
	text.trim()
		.lines()
		.map(|line| {
			let (prefix, middle, suffix) = if let Some(line) = line.strip_prefix("- ")
				&& let Some(line) = line.strip_suffix(" -")
			{
				("**", line, "**")
			} else {
				let prefixes_needing_escaping = ['-', '=', '#', '>'];
				if prefixes_needing_escaping.iter().any(|&c| line.starts_with(c)) {
					("\\", line, "")
				} else {
					("", line, "")
				}
			};

			let middle = markdown_urls(middle);

			if prefix.is_empty() && suffix.is_empty() {
				middle
			} else {
				Cow::Owned(format!("{}{}{}", prefix, middle, suffix))
			}
		})
		.join("\n")
}

fn markdown_urls<'a>(text: impl Into<Cow<'a, str>>) -> Cow<'a, str> {
	let text = text.into();
	let mut result = None;
	let mut current = text.as_ref();

	while !current.is_empty() {
		let h = current.find("http://").map(|i| (i, false));
		let hs = current.find("https://").map(|i| (i, false));
		let a = current.find("<a href=\"").map(|i| (i, true));

		let next = [h, hs, a].into_iter().flatten().min_by_key(|&(i, _)| i);

		let (start, is_a) = match next {
			Some(n) => n,
			None => break,
		};

		let result = result.get_or_insert_with(|| String::with_capacity(text.len() + 128));
		result.push_str(&current[..start]);
		let remainder = &current[start..];

		if is_a {
			let mut consumed = false;
			if let Some(url_end_quote) = remainder[9..].find('"') {
				let url_end_quote = url_end_quote + 9;
				let url = &remainder[9..url_end_quote];
				let rest = &remainder[url_end_quote..];
				if let Some(tag_end) = rest.find('>') {
					let tag_end_abs = url_end_quote + tag_end;
					let rest = &remainder[tag_end_abs + 1..];
					if let Some(closing_tag_start) = rest.find("</a>") {
						let closing_tag_start_abs = tag_end_abs + 1 + closing_tag_start;
						let text = &remainder[tag_end_abs + 1..closing_tag_start_abs];

						result.push('[');
						result.push_str(text);
						result.push_str("](");
						result.push_str(url);
						result.push(')');

						current = &remainder[closing_tag_start_abs + 4..];
						consumed = true;
					}
				}
			}

			if !consumed {
				result.push_str(&remainder[..1]);
				current = &remainder[1..];
			}
		} else {
			let end = remainder.find(|c: char| c.is_whitespace()).unwrap_or(remainder.len());
			let mut url_end = end;

			// backtrack trailing punctuation
			while url_end > 0 {
				let last_char = remainder.as_bytes()[url_end - 1];
				if matches!(
					last_char,
					b'.' | b',' | b';' | b':' | b'!' | b'?' | b')' | b']' | b'}' | b'\'' | b'"'
				) {
					url_end -= 1;
				} else {
					break;
				}
			}

			if url_end > 0 {
				let url = &remainder[..url_end];
				result.push('[');
				result.push_str(url);
				result.push_str("](");
				result.push_str(url);
				result.push(')');
				current = &remainder[url_end..];
			} else {
				// this should be impossible because we found http:// or https://, but let's be safe
				result.push_str(&remainder[..1]);
				current = &remainder[1..];
			}
		}
	}

	if let Some(mut result) = result {
		result.push_str(current);
		Cow::Owned(result)
	} else {
		text
	}
}

#[cfg(test)]
mod tests {
	use slint::StyledText;
	use test_case::test_case;

	#[allow(clippy::zero_prefixed_literal)]
	#[test_case(00, "", "")]
	#[test_case(01, "no formatting", "no formatting")]
	#[test_case(02, "- HEADER -", "**HEADER**")]
	#[test_case(03, "---", "\\---")]
	#[test_case(04, "-----", "\\-----")]
	#[test_case(05, "# Stuff after hash", "\\# Stuff after hash")]
	#[test_case(06, "#### Stuff after hashes", "\\#### Stuff after hashes")]
	#[test_case(07, "> Stuff after block quote", "\\> Stuff after block quote")]
	#[test_case(08, ">>> Stuff after block quotes", "\\>>> Stuff after block quotes")]
	#[test_case(09, "=======================", "\\=======================")]
	#[test_case(10, "<a href=\"https://www.mamedev.org\">MAME</a>", "[MAME](https://www.mamedev.org)")]
	fn markdown_from_history_text(_index: usize, input: &str, expected: &str) {
		let actual = super::markdown_from_history_text(input);
		let _ = StyledText::from_markdown(&actual).expect("markdown_from_history_text() should produce valid markdown");
		assert_eq!(&actual, expected);
	}

	#[allow(clippy::zero_prefixed_literal)]
	#[test_case(00, "no url", "no url")]
	#[test_case(01, "http://google.com", "[http://google.com](http://google.com)")]
	#[test_case(02, "https://google.com", "[https://google.com](https://google.com)")]
	#[test_case(03, "Visit http://google.com for info.", "Visit [http://google.com](http://google.com) for info.")]
	#[test_case(04, "Check (https://www.mamedev.org)", "Check ([https://www.mamedev.org](https://www.mamedev.org))")]
	#[test_case(05, "<a href=\"https://www.mamedev.org\">MAME</a>", "[MAME](https://www.mamedev.org)")]
	#[test_case(06, "Link: <a href=\"http://info.com\">Info</a>.", "Link: [Info](http://info.com).")]
	#[test_case(
		07,
		"Multiple: http://one.com and https://two.com!",
		"Multiple: [http://one.com](http://one.com) and [https://two.com](https://two.com)!"
	)]
	#[test_case(
		08,
		"Mixed: <a href=\"http://google.com\">Google</a> and http://bing.com",
		"Mixed: [Google](http://google.com) and [http://bing.com](http://bing.com)"
	)]
	fn markdown_urls(_index: usize, input: &str, expected: &str) {
		let actual = super::markdown_urls(input);
		assert_eq!(&actual, expected);
	}
}
