use std::borrow::Cow;
use std::env::current_exe;
use std::fs::metadata;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use is_executable::IsExecutable;
use itertools::Itertools;
use tracing::event;
use tracing::Level;

use crate::prefs::PrefsPaths;
use crate::runtime::MameWindowing;

const LOG: Level = Level::DEBUG;

#[derive(Copy, Clone, Debug, strum_macros::Display)]
pub enum PreflightProblem {
	#[strum(to_string = "No MAME executable path specified")]
	NoMameExecutablePath,
	#[strum(to_string = "No MAME executable found")]
	NoMameExecutable,
	#[strum(to_string = "MAME executable file is not executable")]
	MameExecutableIsNotExecutable,
	#[strum(to_string = "No valid plugins paths specified")]
	NoPluginsPaths,
	#[strum(to_string = "MAME boot.lua not found")]
	PluginsBootNotFound,
	#[strum(to_string = "BletchMAME worker_ui plugin not found")]
	WorkerUiPluginNotFound,
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Problems found during MAME preflight: {0:?}")]
	MamePreflightProblems(Vec<PreflightProblem>),
}

#[derive(Clone, Debug)]
pub struct MameArgumentsSource<'a> {
	windowing: &'a MameWindowing,
	mame_executable_path: Option<&'a str>,
	roms_paths: &'a [String],
	samples_paths: &'a [String],
	plugins_paths: &'a [String],
	software_lists_paths: &'a [String],
	cfg_path: &'a [String],
	nvram_path: &'a [String],
}

impl<'a> MameArgumentsSource<'a> {
	pub fn new(prefs_paths: &'a PrefsPaths, windowing: &'a MameWindowing) -> Result<Self> {
		let mame_executable_path = prefs_paths.mame_executable.as_deref();
		let roms_paths = prefs_paths.roms.as_slice();
		let samples_paths = prefs_paths.samples.as_slice();
		let plugins_paths = prefs_paths.plugins.as_slice();
		let software_lists_paths = prefs_paths.software_lists.as_slice();
		let cfg_path: &[String] = prefs_paths.cfg.as_slice();
		let nvram_path = prefs_paths.nvram.as_slice();
		let result: MameArgumentsSource<'a> = Self {
			windowing,
			roms_paths,
			mame_executable_path,
			samples_paths,
			plugins_paths,
			software_lists_paths,
			cfg_path,
			nvram_path,
		};
		Ok(result)
	}

	pub fn preflight(&self) -> Result<()> {
		let results = preflight_checks(self.mame_executable_path, self.plugins_paths, current_exe_lookup);
		if results.is_empty() {
			Ok(())
		} else {
			Err(ThisError::MamePreflightProblems(results).into())
		}
	}
}

#[derive(Debug)]
pub struct MameArguments {
	pub program: String,
	pub args: Vec<Cow<'static, str>>,
}

impl From<MameArgumentsSource<'_>> for MameArguments {
	fn from(value: MameArgumentsSource<'_>) -> Self {
		mame_args_from_source(value, current_exe_lookup)
	}
}

fn current_exe_lookup() -> Option<PathBuf> {
	current_exe().ok()
}

/// Returns platform specific arguments to MAME
fn platform_specific_args() -> Vec<&'static str> {
	if cfg!(target_family = "windows") {
		// Windows MAME
		vec![
			"-keyboardprovider",
			"dinput",
			"-mouseprovider",
			"dinput",
			"-lightgunprovider",
			"dinput",
		]
	} else if cfg!(target_family = "unix") {
		// SDL MAME
		vec!["-video", "soft"]
	} else {
		// Unknown
		vec![]
	}
}

/// FIXME
pub fn preflight_checks_public(
	mame_executable_path: Option<&str>,
	plugins_paths: &[impl AsRef<str>],
) -> Vec<PreflightProblem> {
	preflight_checks(mame_executable_path, plugins_paths, current_exe_lookup)
}

pub fn preflight_checks(
	mame_executable_path: Option<&str>,
	plugins_paths: &[impl AsRef<str>],
	current_exe_lookup: impl Fn() -> Option<PathBuf>,
) -> Vec<PreflightProblem> {
	let mut problems = Vec::new();

	// MAME executable preflights
	if let Some(mame_executable_path) = mame_executable_path {
		let mame_executable_path = Path::new(mame_executable_path);
		let metadata = metadata(mame_executable_path);
		if metadata.is_err() {
			problems.push(PreflightProblem::NoMameExecutable);
		} else if metadata.is_ok_and(|x| !x.is_file()) || !mame_executable_path.is_executable() {
			problems.push(PreflightProblem::MameExecutableIsNotExecutable);
		}
	} else {
		problems.push(PreflightProblem::NoMameExecutablePath)
	}

	// plugins preflights
	let plugins_paths = plugins_paths
		.iter()
		.flat_map(|path| {
			let path = path.as_ref();
			if let Some((var_name, rest)) = get_var_name(path) {
				let var_value = env_lookup(var_name, mame_executable_path, &current_exe_lookup);
				let result = var_value.map(|x| PathBuf::from(format!("{x}{rest}")));
				result.map(Cow::Owned)
			} else {
				Some(Cow::Borrowed(Path::new(path)))
			}
		})
		.filter(|path| metadata(path).is_ok_and(|m| m.is_dir()))
		.collect::<Vec<_>>();
	if !plugins_paths.is_empty() {
		let mut found_boot = false;
		let mut found_worker_ui = false;
		for path in plugins_paths {
			let boot = rel_path(&path, &["boot.lua"]);
			found_boot |= boot.is_file();

			let worker_ui_init = rel_path(&path, &["worker_ui", "init.lua"]);
			let worker_ui_json = rel_path(&path, &["worker_ui", "plugin.json"]);
			found_worker_ui |= worker_ui_init.is_file() && worker_ui_json.is_file();
		}

		if !found_boot {
			problems.push(PreflightProblem::PluginsBootNotFound);
		}

		if !found_worker_ui {
			problems.push(PreflightProblem::WorkerUiPluginNotFound);
		}
	} else {
		problems.push(PreflightProblem::NoPluginsPaths);
	}

	event!(LOG, "preflight_checks(): problems={problems:?}");
	problems
}

fn rel_path(path: &Path, children: &[impl AsRef<Path>]) -> PathBuf {
	let mut path = path.to_path_buf();
	for child in children {
		path.push(child);
	}
	path
}

fn mame_args_from_source(
	source: MameArgumentsSource<'_>,
	current_exe_lookup: impl Fn() -> Option<PathBuf>,
) -> MameArguments {
	// lambda that looks up variables
	let mame_executable_path = source.mame_executable_path;
	let lookup_var = move |var_name: &str| env_lookup(var_name, mame_executable_path, &current_exe_lookup);

	// convert all path vec's to the appropriate MAME arguments
	let paths = [
		("-rompath", source.roms_paths),
		("-samplepath", source.samples_paths),
		("-pluginspath", source.plugins_paths),
		("-hashpath", source.software_lists_paths),
		("-cfg_directory", source.cfg_path),
		("-nvram_directory", source.nvram_path),
	]
	.into_iter()
	.filter(|(_, paths)| !paths.is_empty())
	.map(|(arg, paths)| {
		let paths_str = get_full_path(paths, &lookup_var);
		(arg, paths_str)
	})
	.collect::<Vec<_>>();

	// figure out windowing
	let windowing_args = match source.windowing {
		MameWindowing::Attached(window) => vec!["-attach_window".into(), Cow::Owned(window.to_string())],
		MameWindowing::Windowed => vec!["-w".into(), "-nomax".into()],
		MameWindowing::WindowedMaximized => vec!["-w".into(), "-max".into()],
		MameWindowing::Fullscreen => vec!["-now".into()],
	};

	// platform specific arguments
	let platform_args = platform_specific_args().into_iter().map(Cow::Borrowed);

	// assemble all arguments
	let program = source.mame_executable_path.unwrap().to_string();
	let args = ["-plugin", "worker_ui", "-skip_gameinfo", "-nomouse", "-debug"]
		.into_iter()
		.map(Cow::Borrowed)
		.chain(windowing_args)
		.chain(platform_args)
		.chain(
			paths
				.into_iter()
				.flat_map(|(arg, path)| [Cow::Borrowed(arg), Cow::Owned(path)]),
		)
		.collect::<Vec<_>>();
	MameArguments { program, args }
}

fn get_full_path(paths: &[impl AsRef<str>], lookup_var: impl Fn(&str) -> Option<String>) -> String {
	paths
		.iter()
		.flat_map(|path| {
			let path = path.as_ref();
			if let Some((var_name, rest)) = get_var_name(path) {
				let var_value = lookup_var(var_name);
				let result = var_value.map(|x| format!("{x}{rest}"));
				event!(LOG, "get_full_path(): path={path:?} result={result:?}");
				result.map(Cow::Owned)
			} else {
				Some(Cow::Borrowed(path))
			}
		})
		.join(";")
}

fn get_var_name(s: &str) -> Option<(&str, &str)> {
	let s = s.strip_prefix("$(")?;
	let idx = s.find(')')?;
	let var_name = &s[0..idx];
	let rest = &s[(idx + 1)..];
	Some((var_name, rest))
}

fn env_lookup(
	var_name: &str,
	mame_executable_path: Option<&str>,
	current_exe_lookup: impl Fn() -> Option<PathBuf>,
) -> Option<String> {
	let file_path = match var_name {
		"MAMEPATH" => mame_executable_path.map(|x| Path::new(x).to_path_buf()),
		"BLETCHMAMEPATH" => current_exe_lookup(),
		_ => None,
	}?;
	file_path.parent().and_then(|x| x.to_str()).map(|x| x.to_string())
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use crate::runtime::MameWindowing;

	use super::MameArgumentsSource;

	#[test_case(0, &["/foo"], "/foo")]
	#[test_case(1, &["/foo", "/bar"], "/foo;/bar")]
	#[test_case(2, &["/foo", "/bar", "/baz"], "/foo;/bar;/baz")]
	#[test_case(3, &["$(FOO)", "/bar", "/baz"], "/path/foo;/bar;/baz")]
	#[test_case(4, &["/foo", "$(BAR)", "/baz"], "/foo;/path/bar;/baz")]
	#[test_case(5, &["/foo", "$(INVALID)", "/baz"], "/foo;/baz")]
	#[test_case(6, &["$(FOO)/bar", "/baz"], "/path/foo/bar;/baz")]
	pub fn get_full_path(_index: usize, paths: &[&str], expected: &str) {
		let actual = super::get_full_path(paths, |var| match var {
			"FOO" => Some("/path/foo".into()),
			"BAR" => Some("/path/bar".into()),
			_ => None,
		});
		assert_eq!(expected, actual);
	}

	#[test_case(0, "", None)]
	#[test_case(1, "foo", None)]
	#[test_case(2, "foo/bar", None)]
	#[test_case(3, "$(FOO)/bar", Some(("FOO", "/bar")))]
	pub fn get_var_name(_index: usize, s: &str, expected: Option<(&str, &str)>) {
		let actual = super::get_var_name(s);
		assert_eq!(expected, actual)
	}

	#[test]
	pub fn mame_args_from_source() {
		let windowing = MameWindowing::Attached("1234".to_string());
		let source = MameArgumentsSource {
			windowing: &windowing,
			mame_executable_path: Some("/mydir/mame/mame.exe"),
			roms_paths: &["/mydir/mame/roms1".to_string(), "/mydir/mame/roms2".to_string()],
			samples_paths: &["/mydir/mame/samples1".to_string(), "/mydir/mame/samples2".to_string()],
			plugins_paths: &[
				"$(MAMEPATH)/plugins".to_string(),
				"$(BLETCHMAMEPATH)/plugins".to_string(),
			],
			software_lists_paths: &["/mydir/mame/hash".to_string()],
			cfg_path: &["/mydir/mame/cfg".to_string()],
			nvram_path: &["/mydir/mame/nvram".to_string()],
		};
		let result = super::mame_args_from_source(source, || Some(std::path::PathBuf::from("/bmdir/bletchmame")));

		fn find_arg<'a>(args: &'a [impl AsRef<str>], target: &str) -> Option<&'a str> {
			args.iter()
				.position(|x| x.as_ref() == target)
				.map(|idx| args[idx + 1].as_ref())
		}

		let actual = (
			result.program.as_str(),
			find_arg(&result.args, "-attach_window"),
			find_arg(&result.args, "-rompath"),
			find_arg(&result.args, "-samplepath"),
			find_arg(&result.args, "-pluginspath"),
			find_arg(&result.args, "-hashpath"),
			find_arg(&result.args, "-cfg_directory"),
			find_arg(&result.args, "-nvram_directory"),
		);
		let expected = (
			"/mydir/mame/mame.exe",
			Some("1234"),
			Some("/mydir/mame/roms1;/mydir/mame/roms2"),
			Some("/mydir/mame/samples1;/mydir/mame/samples2"),
			Some("/mydir/mame/plugins;/bmdir/plugins"),
			Some("/mydir/mame/hash"),
			Some("/mydir/mame/cfg"),
			Some("/mydir/mame/nvram"),
		);
		assert_eq!(expected, actual);
	}
}
