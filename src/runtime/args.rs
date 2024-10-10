use std::borrow::Cow;
use std::fs::metadata;

use crate::prefs::PrefsPaths;
use crate::runtime::MameWindowing;
use crate::Error;
use crate::Result;

#[derive(Clone, Copy, Debug)]
pub struct MameArgumentsSource<'a> {
	pub windowing: &'a MameWindowing,
	pub mame_executable_path: &'a str,
	pub roms_paths: &'a [String],
	pub samples_paths: &'a [String],
	pub plugins_paths: &'a [String],
}

impl<'a> MameArgumentsSource<'a> {
	pub fn from_prefs(prefs_paths: &'a PrefsPaths, windowing: &'a MameWindowing) -> Result<Self> {
		let Some(mame_executable_path) = prefs_paths.mame_executable.as_ref() else {
			return Err(Error::NoMamePathSpecified.into());
		};
		let roms_paths = prefs_paths.roms.as_slice();
		let samples_paths = prefs_paths.samples.as_slice();
		let plugins_paths = prefs_paths.plugins.as_slice();
		let result = Self {
			windowing,
			roms_paths,
			mame_executable_path,
			samples_paths,
			plugins_paths,
		};
		Ok(result)
	}

	pub fn preflight(&self) -> Result<()> {
		let mame_metadata = metadata(self.mame_executable_path).map_err(|e| Error::CannotFindMame(Box::new(e)))?;
		if !mame_metadata.is_file() {
			return Err(Error::MameIsNotAFile.into());
		}

		Ok(())
	}
}

#[derive(Debug)]
pub struct MameArguments {
	pub program: String,
	pub args: Vec<Cow<'static, str>>,
}

impl From<MameArgumentsSource<'_>> for MameArguments {
	fn from(value: MameArgumentsSource<'_>) -> Self {
		// convert all path vec's to the appropriate MAME arguments
		let paths = [
			("-rompath", value.roms_paths),
			("-samplepath", value.samples_paths),
			("-pluginspath", value.plugins_paths),
		]
		.into_iter()
		.filter(|(_, paths)| !paths.is_empty())
		.map(|(arg, paths)| {
			let paths_str = paths.join(";");
			(arg, paths_str)
		})
		.collect::<Vec<_>>();

		// figure out windowing
		let windowing_args = match value.windowing {
			MameWindowing::Attached(window) => vec!["-attach_window".into(), Cow::Owned(window.to_string())],
			MameWindowing::Windowed => vec!["-w".into(), "-nomax".into()],
			MameWindowing::WindowedMaximized => vec!["-w".into(), "-max".into()],
			MameWindowing::Fullscreen => vec!["-now".into()],
		};

		// platform specific arguments
		let keyboard_provider = cfg!(windows).then_some("dinput");
		let mouse_provider = cfg!(windows).then_some("dinput");
		let lightgun_provider = cfg!(windows).then_some("dinput");
		let platform_args = keyboard_provider
			.iter()
			.flat_map(|x| ["-keyboardprovider", x])
			.chain(mouse_provider.iter().flat_map(|x| ["-mouseprovider", x]))
			.chain(lightgun_provider.iter().flat_map(|x| ["-lightgunprovider", x]))
			.map(Cow::Borrowed);

		// assemble all arguments
		let program = value.mame_executable_path.to_string();
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
		Self { program, args }
	}
}
