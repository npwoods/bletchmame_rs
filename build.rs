use std::env;

use winresource::WindowsResource;

fn main() -> std::io::Result<()> {
	// constants
	let icon_file = "ui/bletchmame.ico";

	// dependencies
	println!("cargo::rerun-if-changed={}", icon_file);

	// build Slint stuff
	slint_build::compile_with_config(
		"ui/main.slint",
		slint_build::CompilerConfiguration::new().with_library_paths(vivi_ui::import_paths()),
	)
	.unwrap();

	// BletchMAME icon
	if env::var_os("CARGO_CFG_WINDOWS").is_some() {
		WindowsResource::new().set_icon(icon_file).compile()?;
	}
	Ok(())
}
