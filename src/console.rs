use std::fs::File;
use std::io::Write;
use std::process::Child;

use anyhow::Result;
use strum::EnumProperty;

use crate::platform::console_init;

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
	pub fn ansi_code(&self) -> &'static str {
		self.get_str("AnsiCode").unwrap()
	}
}

impl Console {
	pub fn new() -> Result<Self> {
		let (process, pipe_file) = console_init("MAME Console")?;
		Ok(Self { process, pipe_file })
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
