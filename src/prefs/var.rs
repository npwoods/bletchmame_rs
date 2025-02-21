use std::borrow::Cow;
use std::env::current_exe;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::path::is_separator;

pub fn resolve_path<'a>(path: &'a str, mame_executable_path: Option<&str>) -> Option<Cow<'a, Path>> {
	let lookup_var = |var_name| env_lookup(var_name, mame_executable_path, current_exe_lookup);
	resolve_path_core(path, &lookup_var)
}

pub fn resolve_paths_string(paths: &[impl AsRef<str>], mame_executable_path: Option<&str>) -> Option<OsString> {
	let lookup_var = |var_name| env_lookup(var_name, mame_executable_path, current_exe_lookup);
	resolve_paths_string_core(paths, &lookup_var)
}

fn resolve_paths_string_core<'a>(
	paths: &'a [impl AsRef<str>],
	lookup_var: &impl Fn(&'a str) -> Option<PathBuf>,
) -> Option<OsString> {
	paths
		.iter()
		.flat_map(|path| {
			let path = path.as_ref();
			resolve_path_core(path, &lookup_var)
		})
		.fold(None, |acc, x| {
			// rewrite when `Join` trait stabilizes
			let mut acc = acc.unwrap_or_default();
			if !acc.is_empty() {
				acc.push(";");
			}
			acc.push(x.as_ref());
			Some(acc)
		})
}

fn resolve_path_core<'a, 'b>(path: &'b str, lookup_var: &impl Fn(&'a str) -> Option<PathBuf>) -> Option<Cow<'b, Path>>
where
	'b: 'a,
{
	if let Some((var_name, rest)) = get_var_name(path) {
		let mut result = lookup_var(var_name)?;
		result.push(rest);
		Some(Cow::Owned(result))
	} else {
		Some(Cow::Borrowed(Path::new(path)))
	}
}

fn get_var_name(s: &str) -> Option<(&str, &str)> {
	let s = s.strip_prefix("$(")?;
	let idx = s.find(')')?;
	let var_name = &s[0..idx];
	let rest = &s[(idx + 1)..].trim_start_matches(is_separator);
	Some((var_name, rest))
}

fn env_lookup(
	var_name: &str,
	mame_executable_path: Option<&str>,
	current_exe_lookup: impl Fn() -> Option<PathBuf>,
) -> Option<PathBuf> {
	let file_path = match var_name {
		"MAMEPATH" => mame_executable_path.map(Path::new).map(Cow::Borrowed),
		"BLETCHMAMEPATH" => current_exe_lookup().map(Cow::Owned),
		_ => None,
	}?;
	file_path.parent().map(|x| x.to_path_buf())
}

fn current_exe_lookup() -> Option<PathBuf> {
	current_exe().ok()
}

#[cfg(test)]
mod test {
	use std::path::MAIN_SEPARATOR_STR;
	use std::path::Path;

	use test_case::test_case;

	#[test_case(0, "/foo", Some("/foo"))]
	#[test_case(1, "$(FOO)", Some("/path/foo"))]
	#[test_case(2, "$(FOO)/bar", Some("/path/foo/bar"))]
	pub fn resolve_path_core(_index: usize, path: &str, expected: Option<impl AsRef<Path>>) {
		let actual = super::resolve_path_core(path, &|var| match var {
			"FOO" => Some("/path/foo".into()),
			"BAR" => Some("/path/bar".into()),
			_ => None,
		});
		let actual = actual.as_ref().map(|x| x.as_ref());
		let expected = expected.as_ref().map(|x| x.as_ref());
		assert_eq!(expected, actual);
	}

	#[test_case(0, &["/foo"], "/foo")]
	#[test_case(1, &["/foo", "/bar"], "/foo;/bar")]
	#[test_case(2, &["/foo", "/bar", "/baz"], "/foo;/bar;/baz")]
	#[test_case(3, &["$(FOO)", "/bar", "/baz"], "/path/foo/;/bar;/baz")]
	#[test_case(4, &["/foo", "$(BAR)", "/baz"], "/foo;/path/bar/;/baz")]
	#[test_case(5, &["/foo", "$(INVALID)", "/baz"], "/foo;/baz")]
	#[test_case(6, &["$(FOO)/bar", "/baz"], "/path/foo/bar;/baz")]
	pub fn resolve_paths_string_core(_index: usize, paths: &[&str], expected: &str) {
		let actual = super::resolve_paths_string_core(paths, &|var| match var {
			"FOO" => Some("/path/foo".into()),
			"BAR" => Some("/path/bar".into()),
			_ => None,
		});
		let actual = actual.map(|x| x.to_str().unwrap().replace(MAIN_SEPARATOR_STR, "/"));

		let expected = Some(expected.into());
		assert_eq!(expected, actual);
	}

	#[test_case(0, "", None)]
	#[test_case(1, "foo", None)]
	#[test_case(2, "foo/bar", None)]
	#[test_case(3, "$(FOO)/bar", Some(("FOO", "bar")))]
	pub fn get_var_name(_index: usize, s: &str, expected: Option<(&str, &str)>) {
		let actual = super::get_var_name(s);
		assert_eq!(expected, actual)
	}
}
