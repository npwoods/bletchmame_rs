use std::borrow::Cow;
use std::ffi::OsStr;
use std::ffi::OsString;

use strum::IntoEnumIterator;

use crate::prefs::PrefsPaths;
use crate::prefs::pathtype::PathType;
use crate::runtime::MameWindowing;

#[derive(Debug)]
pub struct MameArguments {
	pub program: String,
	pub args: Vec<Cow<'static, OsStr>>,
}

impl MameArguments {
	pub fn new(prefs_paths: &PrefsPaths, windowing: &MameWindowing) -> Self {
		// convert all path vec's to the appropriate MAME arguments
		let path_args = PathType::iter()
			.filter_map(|path_type| {
				let arg = path_type.mame_argument()?;
				let value = prefs_paths.full_string(path_type)?;
				Some((arg, value))
			})
			.flat_map(|(arg, value)| [Cow::Borrowed(OsStr::new(arg)), Cow::Owned(value)]);

		// figure out windowing
		let windowing_args = match windowing {
			MameWindowing::Attached(window) => vec!["-attach_window".into(), Cow::Owned(window.to_string())],
			MameWindowing::Windowed => vec!["-w".into(), "-nomax".into()],
			MameWindowing::WindowedMaximized => vec!["-w".into(), "-max".into()],
			MameWindowing::Fullscreen => vec!["-now".into()],
		};

		// platform specific arguments
		let platform_args = platform_specific_args().into_iter().map(Cow::Borrowed);

		// assemble all arguments
		let program = prefs_paths.mame_executable.as_ref().unwrap().to_string();
		let args = ["-plugin", "worker_ui", "-skip_gameinfo", "-nomouse", "-debug"]
			.into_iter()
			.map(Cow::Borrowed)
			.chain(windowing_args)
			.chain(platform_args)
			.map(|x| match x {
				Cow::Borrowed(x) => Cow::Borrowed(OsStr::new(x)),
				Cow::Owned(x) => Cow::Owned(OsString::from(x)),
			})
			.chain(path_args)
			.collect::<Vec<_>>();
		Self { program, args }
	}
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

#[cfg(test)]
mod test {
	use std::path::MAIN_SEPARATOR_STR;
	use std::path::Path;

	use crate::prefs::PrefsPaths;
	use crate::runtime::MameWindowing;

	use super::MameArguments;

	#[test]
	pub fn mame_args_new() {
		let windowing = MameWindowing::Attached("1234".into());
		let prefs_paths = PrefsPaths {
			mame_executable: Some("/mydir/mame/mame.exe".into()),
			roms: vec!["/mydir/mame/roms1".into(), "/mydir/mame/roms2".into()],
			samples: vec!["/mydir/mame/samples1".into(), "/mydir/mame/samples2".into()],
			plugins: vec!["$(MAMEPATH)/plugins".into()],
			software_lists: vec!["/mydir/mame/hash".into()],
			cfg: Some("/mydir/mame/cfg".into()),
			nvram: Some("/mydir/mame/nvram".into()),
			cheats: Some("/mydir/mame/cheats".into()),
			snapshots: vec!["/mydir/mame/snapshots".into()],
		};
		let result = MameArguments::new(&prefs_paths, &windowing);

		fn find_arg(args: &[impl AsRef<Path>], target: &str) -> Option<String> {
			args.iter().position(|x| x.as_ref() == Path::new(target)).map(|idx| {
				args[idx + 1]
					.as_ref()
					.to_str()
					.unwrap()
					.replace(MAIN_SEPARATOR_STR, "/")
			})
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
			Some("1234".into()),
			Some("/mydir/mame/roms1;/mydir/mame/roms2".into()),
			Some("/mydir/mame/samples1;/mydir/mame/samples2".into()),
			Some("/mydir/mame/plugins".into()),
			Some("/mydir/mame/hash".into()),
			Some("/mydir/mame/cfg".into()),
			Some("/mydir/mame/nvram".into()),
		);
		assert_eq!(expected, actual);
	}
}
