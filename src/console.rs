use std::fs::File;
use std::io::Write;
use std::process::Child;

use anyhow::Result;
use strum::EnumProperty;

pub struct Console {
	process: Child,
	pipe_file: File,
}

#[derive(Debug, EnumProperty)]
pub enum EmitType {
	#[strum(props(AnsiCode = "\x1B[1;37m"))]
	CommandLine,
	#[strum(props(AnsiCode = "\x1B[1;33m"))]
	Command,
	#[strum(props(AnsiCode = "\x1B[1;32m"))]
	Response,
	#[strum(props(AnsiCode = "\x1B[0;37m"))]
	Cruft,
}

impl EmitType {
	fn ansi_code(&self) -> &'static str {
		self.get_str("AnsiCode").unwrap()
	}
}

impl Console {
	#[cfg(target_os = "windows")]
	pub fn new() -> Result<Self> {
		use std::process::Command;
		use std::process::Stdio;

		use uuid::Uuid;

		use crate::platform::CommandExt;
		use crate::platform::WinNamedPipe;

		let exe_path = std::env::current_exe()?;

		let guid = Uuid::new_v4();
		let pipe_name = format!("\\\\.\\pipe\\bletchmame_pipe_{guid}");
		let pipe = WinNamedPipe::new(&pipe_name)?;

		// launch a new process with the --echo-console argument
		let process = Command::new(exe_path)
			.arg("--echo-console")
			.arg(pipe_name)
			.stdin(Stdio::null())
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.create_new_console()
			.spawn()?;

		// create the file
		let mut pipe_file = pipe.connect()?;

		// set the title
		let _ = write!(pipe_file, "\x1B]0;MAME Console\x07");

		// and set us up
		Ok(Self { process, pipe_file })
	}

	#[cfg(not(target_os = "windows"))]
	pub fn new() -> Result<Self> {
		todo!()
	}

	pub fn emit(&mut self, emit_type: EmitType, data: &str) -> Result<()> {
		let ansi_code = emit_type.ansi_code();
		Ok(writeln!(self.pipe_file, "{ansi_code}{data}")?)
	}

	pub fn is_running(&mut self) -> bool {
		self.process.try_wait().is_ok_and(|x| x.is_none())
	}
}

impl Drop for Console {
	fn drop(&mut self) {
		let _ = self.process.kill();
		let _ = self.process.wait();
	}
}
