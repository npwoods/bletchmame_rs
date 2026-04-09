use std::fs::File;
use std::io::Write;
use std::process::Child;

use anyhow::Result;
use console::Style;

use crate::platform::interaction_monitor_init;

pub struct InteractionMonitor {
	process: Child,
	pipe_file: File,
}

#[derive(Debug)]
pub enum EmitType {
	CommandLine,
	Command,
	Response,
	Cruft,
}

impl EmitType {
	pub fn style(&self) -> Style {
		match self {
			EmitType::CommandLine => Style::new().white().bold(),
			EmitType::Command => Style::new().yellow().bold(),
			EmitType::Response => Style::new().green().bold(),
			EmitType::Cruft => Style::new().white(),
		}
	}
}

impl InteractionMonitor {
	pub fn new() -> Result<Self> {
		let (process, pipe_file) = interaction_monitor_init("MAME Interaction Monitor")?;
		Ok(Self { process, pipe_file })
	}

	pub fn emit(&mut self, emit_type: EmitType, data: &str) -> Result<()> {
		let style = emit_type.style();
		Ok(writeln!(self.pipe_file, "{}", style.apply_to(data))?)
	}

	pub fn is_running(&mut self) -> bool {
		self.process.try_wait().is_ok_and(|x| x.is_none())
	}
}

impl Drop for InteractionMonitor {
	fn drop(&mut self) {
		let _ = self.process.kill();
		let _ = self.process.wait();
	}
}
