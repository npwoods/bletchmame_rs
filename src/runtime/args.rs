use std::borrow::Cow;
use std::fs::metadata;

use crate::prefs::PrefsPaths;
use crate::Error;
use crate::Result;

#[derive(Clone, Copy, Debug)]
pub struct MameArgumentsSource<'a> {
	pub mame_executable_path: &'a str,
	pub roms_paths: &'a [String],
	pub samples_paths: &'a [String],
	pub plugins_paths: &'a [String],
}

impl<'a> MameArgumentsSource<'a> {
	pub fn from_prefs(prefs_paths: &'a PrefsPaths) -> Result<Self> {
		let Some(mame_executable_path) = prefs_paths.mame_executable.as_ref() else {
			return Err(Error::NoMamePathSpecified.into());
		};
		let roms_paths = prefs_paths.roms.as_slice();
		let samples_paths = prefs_paths.samples.as_slice();
		let plugins_paths = prefs_paths.plugins.as_slice();
		let result = Self {
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

		// assemble all arguments
		let program = value.mame_executable_path.to_string();
		let args = ["-plugin", "worker_ui", "-w", "-nomax"]
			.into_iter()
			.map(Cow::Borrowed)
			.chain(
				paths
					.into_iter()
					.flat_map(|(arg, path)| [Cow::Borrowed(arg), Cow::Owned(path)]),
			)
			.collect::<Vec<_>>();
		Self { program, args }
	}
}
