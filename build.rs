fn main() {
	slint_build::compile_with_config(
		"ui/main.slint",
		slint_build::CompilerConfiguration::new().with_library_paths(vivi_ui::import_paths()),
	)
	.unwrap();
}
