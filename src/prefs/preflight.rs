use std::fs::metadata;
use std::path::Path;
use std::path::PathBuf;

use is_executable::IsExecutable;
use tracing::info;

use crate::prefs::PreflightProblem;

pub fn preflight_checks<T>(
	mame_executable_path: Option<&Path>,
	plugins_path_iter: impl Iterator<Item = T>,
) -> Vec<PreflightProblem>
where
	T: AsRef<Path>,
{
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
	let plugins_paths = plugins_path_iter
		.filter(|path| metadata(path).is_ok_and(|m| m.is_dir()))
		.collect::<Vec<_>>();
	if !plugins_paths.is_empty() {
		let mut found_boot = false;
		let mut found_worker_ui = false;
		for path in plugins_paths {
			let path = path.as_ref();
			let boot = rel_path(path, &["boot.lua"]);
			found_boot |= boot.is_file();

			let worker_ui_init = rel_path(path, &["worker_ui", "init.lua"]);
			let worker_ui_json = rel_path(path, &["worker_ui", "plugin.json"]);
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

	info!(problems=?problems, "preflight_checks()");
	problems
}

fn rel_path(path: &Path, children: &[impl AsRef<Path>]) -> PathBuf {
	let mut path = path.to_path_buf();
	for child in children {
		path.push(child);
	}
	path
}

#[cfg(test)]
mod test {
	use strum::IntoEnumIterator;

	use crate::prefs::PreflightProblem;

	#[test]
	fn preflight_problem_type() {
		let _ = PreflightProblem::iter().map(|x| x.problem_type()).collect::<Vec<_>>();
	}
}
