use binary_serde::BinarySerde;

#[derive(Clone, Copy, Debug, Default, BinarySerde)]
pub struct Header {
	pub magic: [u8; 8],
	pub sizes_hash: u64,
	pub build_strindex: u32,
	pub machine_count: u32,
}

#[derive(Clone, Copy, Debug, Default, BinarySerde)]
pub struct Machine {
	pub name_strindex: u32,
	pub source_file_strindex: u32,
	pub description_strindex: u32,
	pub year_strindex: u32,
	pub manufacturer_strindex: u32,
	pub runnable: bool,
}
