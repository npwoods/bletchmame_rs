use std::borrow::Cow;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::rc::Rc;
use std::sync::Arc;

use smol_str::SmolStr;
use strum::IntoEnumIterator;

use crate::prefs::Preferences;
use crate::prefs::PreflightProblem;
use crate::prefs::pathtype::PathType;
use crate::runtime::MameWindowing;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MameArguments {
	pub program: SmolStr,
	pub args: Arc<[Cow<'static, OsStr>]>,
}

impl MameArguments {
	pub fn new(prefs: &Preferences, windowing: &MameWindowing, skip_file_system_checks: bool) -> MameArgumentsResult {
		// run preflight first
		let preflight_problems = prefs.paths.preflight(skip_file_system_checks);
		if !preflight_problems.is_empty() {
			let program = prefs.paths.mame_executable.clone();
			let preflight_problems = preflight_problems.into();
			let error = MameArgumentsError {
				program,
				preflight_problems,
			};
			return Err(error);
		}

		// convert all path vec's to the appropriate MAME arguments
		let path_args = PathType::iter()
			.filter_map(|path_type| {
				let arg = path_type.mame_argument()?;
				let value = prefs.paths.full_string(path_type)?;
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

		// video arguments
		let video_args = ["-prescale".into(), Cow::Owned(prefs.prescale.to_string())].into_iter();

		// platform specific arguments
		let platform_args = platform_specific_args().into_iter().map(Cow::Borrowed);

		// extra arguments
		let extra_mame_arguments = prefs
			.extra_mame_arguments
			.split_whitespace()
			.map(|s| Cow::Owned(OsString::from(s)));

		// assemble all arguments
		let program = prefs.paths.mame_executable.clone().unwrap();
		let args = ["-plugin", "worker_ui", "-skip_gameinfo", "-nomouse", "-debug"]
			.into_iter()
			.map(Cow::Borrowed)
			.chain(windowing_args)
			.chain(platform_args)
			.chain(video_args)
			.map(|x| match x {
				Cow::Borrowed(x) => Cow::Borrowed(OsStr::new(x)),
				Cow::Owned(x) => Cow::Owned(OsString::from(x)),
			})
			.chain(path_args)
			.chain(extra_mame_arguments)
			.collect::<Arc<[_]>>();
		Ok(Self { program, args })
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MameArgumentsError {
	pub program: Option<SmolStr>,
	pub preflight_problems: Rc<[PreflightProblem]>,
}

pub type MameArgumentsResult = std::result::Result<MameArguments, MameArgumentsError>;

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

	use crate::prefs::Preferences;
	use crate::prefs::PrefsPaths;
	use crate::runtime::MameWindowing;

	use super::MameArguments;

	#[test]
	pub fn mame_args_new() {
		let windowing = MameWindowing::Attached("1234".into());
		let paths = PrefsPaths {
			mame_executable: Some("/mydir/mame/mame.exe".into()),
			roms: vec!["/mydir/mame/roms1".into(), "/mydir/mame/roms2".into()],
			samples: vec!["/mydir/mame/samples1".into(), "/mydir/mame/samples2".into()],
			plugins: vec!["$(MAMEPATH)/plugins".into()],
			software_lists: vec!["/mydir/mame/hash".into()],
			cfg: Some("/mydir/mame/cfg".into()),
			nvram: Some("/mydir/mame/nvram".into()),
			cheats: Some("/mydir/mame/cheats".into()),
			..Default::default()
		};
		let paths = paths.into();
		let prefs = Preferences {
			paths,
			..Default::default()
		};
		let result = MameArguments::new(&prefs, &windowing, true).unwrap();

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
