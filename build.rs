use std::env;
use std::error::Error;
use std::fs::File;
use std::path::PathBuf;

use ico::IconDirEntry;
use ico::IconImage;
use winresource::WindowsResource;

fn main() -> std::io::Result<()> {
	// constants
	let icon_png = "ui/bletchmame.png";

	// dependencies
	println!("cargo::rerun-if-changed={}", icon_png);

	// set the experimental environment variable
	unsafe {
		env::set_var("SLINT_ENABLE_EXPERIMENTAL_FEATURES", "1");
	}

	// build Slint stuff
	slint_build::compile_with_config(
		"ui/main.slint",
		slint_build::CompilerConfiguration::new().with_library_paths(vivi_ui::import_paths()),
	)
	.unwrap();

	// Qt interop stuff
	#[cfg(feature = "slint-qt-backend")]
	{
		let dep_qt_include_path = env::var("DEP_QT_INCLUDE_PATH").unwrap();
		let dep_qt_compile_flags = env::var("DEP_QT_COMPILE_FLAGS").unwrap();

		let mut config = cpp_build::Config::new();
		config.flag_if_supported("-Werror");
		config.flag_if_supported("-std=c++17");
		config.flag_if_supported("/std:c++17");
		config.include(dep_qt_include_path);
		for f in dep_qt_compile_flags.split_terminator(";") {
			config.flag(f);
		}
		config.build("src/main.rs");
	}

	// BletchMAME icon
	if env::var_os("CARGO_CFG_WINDOWS").is_some() {
		// convert PNG to ICO
		let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
		let icon_ico = out_dir.join("bletchmame.ico").to_string_lossy().into_owned();
		convert_png_to_ico(icon_png, &icon_ico).unwrap();

		// and embed it
		WindowsResource::new().set_icon(&icon_ico).compile()?;
	}
	Ok(())
}

fn convert_png_to_ico(input_path: &str, output_path: &str) -> Result<(), Box<dyn Error>> {
	// create a new, empty icon collection:
	let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);

	// read a PNG file from disk and add it to the collection:
	let file = File::open(input_path)?;
	let image = IconImage::read_png(file)?;
	icon_dir.add_entry(IconDirEntry::encode(&image)?);

	// finally, write the ICO file to disk:
	let file = File::create(output_path)?;
	icon_dir.write(file)?;
	Ok(())
}
