use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs;
use std::fs::File;
use std::path::PathBuf;

use ico::IconDirEntry;
use ico::IconImage;
use winresource::WindowsResource;

fn main() -> Result<(), Box<dyn Error>> {
	// constants
	let icon_png = "ui/icons/bletchmame.png";

	// dependencies
	println!("cargo::rerun-if-changed={icon_png}");

	// build library paths
	let slint_material_components_dir = slint_material_components::import_path()
		.get("slint")
		.unwrap()
		.join("..")
		.join("src");
	let library_paths = HashMap::from([("slint".into(), slint_material_components_dir)]);

	// build Slint stuff
	slint_build::compile_with_config(
		"ui/main.slint",
		slint_build::CompilerConfiguration::new().with_library_paths(library_paths),
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

	// we like it when there is a plugins directory next to the BletchMAME executable
	create_symlink_to_plugins_directory()?;

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

fn create_symlink_to_plugins_directory() -> Result<(), Box<dyn Error>> {
	let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
	let plugins_src = manifest_dir.join("plugins");

	let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
	let mut target_dir = out_dir;
	while target_dir.file_name().and_then(|s| s.to_str()) != Some("build") {
		if !target_dir.pop() {
			break;
		}
	}
	target_dir.pop(); // target/debug or target/release

	let plugins_dst = target_dir.join("plugins");
	if fs::symlink_metadata(&plugins_dst).is_err() {
		let result: Result<(), String> = {
			#[cfg(windows)]
			{
				let result = std::process::Command::new("cmd")
					.arg("/c")
					.arg("mklink")
					.arg("/j")
					.arg(&plugins_dst)
					.arg(&plugins_src)
					.status();
				match result {
					Ok(s) if s.success() => Ok(()),
					Ok(s) => Err(format!("mklink command failed with exit code: {}", s)),
					Err(e) => Err(format!("Failed to execute mklink command: {}", e)),
				}
			}

			#[cfg(unix)]
			{
				std::os::unix::fs::symlink(&plugins_src, &plugins_dst)
					.map_err(|e| format!("Failed to create symlink for plugins: {}", e))
			}

			#[cfg(not(any(windows, unix)))]
			Ok(())
		};
		if let Err(e) = result {
			eprintln!("cargo:warning={}. The application might not find its plugins", e);
		}
	};
	Ok(())
}
