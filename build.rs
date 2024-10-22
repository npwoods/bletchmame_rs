use std::env;

use winresource::WindowsResource;

fn main() -> std::io::Result<()> {
	// build Slint stuff
	slint_build::compile_with_config(
		"ui/main.slint",
		slint_build::CompilerConfiguration::new().with_library_paths(vivi_ui::import_paths()),
	)
	.unwrap();

	if env::var_os("CARGO_CFG_WINDOWS").is_some() {
		WindowsResource::new()
			// This path can be absolute, or relative to your crate root.
			.set_icon("ui/bletchmame.ico")
			.compile()?;
	}
	Ok(())
}
