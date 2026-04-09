use std::fs::File;
use std::fs::OpenOptions;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;

use anyhow::Result;
use uuid::Uuid;

pub fn unix_interaction_monitor_init(title: &str) -> Result<(Child, File)> {
	// create a temp file for console output and spawn xterm to tail it
	let mut path = std::env::temp_dir();
	let filename = format!("bletchmame_console_{0}", Uuid::new_v4());
	path.push(filename);
	let path_str = path.to_string_lossy().into_owned();

	// create the file that xterm will tail
	let pipe_file = OpenOptions::new().create(true).append(true).open(&path)?;

	// spawn xterm to display the file contents
	let process = Command::new("xterm")
		.arg("-T")
		.arg(title)
		.arg("-bg")
		.arg("black")
		.arg("-e")
		.arg("tail")
		.arg("-f")
		.arg(&path_str)
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()?;

	Ok((process, pipe_file))
}
