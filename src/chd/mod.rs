//! Limited support for MAME's CHD file format (nothing more than extracting
//! SHA-1 hashes)
use std::io::Read;

use anyhow::Result;
use zerocopy::BigEndian;
use zerocopy::FromBytes;
use zerocopy::Immutable;
use zerocopy::KnownLayout;

use crate::assethash::AssetHash;

type U32 = zerocopy::byteorder::U32<BigEndian>;

#[repr(C, packed)]
#[derive(FromBytes, Immutable, KnownLayout)]
struct ChdHeaderBegin {
	tag: [u8; 8],
	length: U32,
	version: U32,
}

#[repr(C, packed)]
#[derive(FromBytes, Immutable, KnownLayout)]
struct ChdHeaderV3 {
	_begin: ChdHeaderBegin,
	_padding: [u8; 64],
	sha1: [u8; 20],
}

#[repr(C, packed)]
#[derive(FromBytes, Immutable, KnownLayout)]
struct ChdHeaderV4 {
	_begin: ChdHeaderBegin,
	_padding: [u8; 32],
	sha1: [u8; 20],
}

#[repr(C, packed)]
#[derive(FromBytes, Immutable, KnownLayout)]
struct ChdHeaderV5 {
	_begin: ChdHeaderBegin,
	_padding: [u8; 68],
	sha1: [u8; 20],
}

#[derive(thiserror::Error, Debug)]
enum ThisError {
	#[error("Invalid CHD Magic Number")]
	InvalidChdMagicNumber,
	#[error("Unknown CHD Version")]
	UnknownChdVersion,
	#[error("Invalid CHD Header")]
	InvalidChdHeader,
}

pub fn chd_asset_hash(file: impl Read) -> Result<AssetHash> {
	// what is the maximum size of our header?
	let max_header_size = [
		size_of::<ChdHeaderV3>(),
		size_of::<ChdHeaderV4>(),
		size_of::<ChdHeaderV5>(),
	]
	.into_iter()
	.max()
	.unwrap();

	// read into a buffer
	let mut buffer = Vec::with_capacity(max_header_size);
	let limit = u64::try_from(max_header_size).unwrap();
	file.take(limit).read_to_end(&mut buffer)?;

	// access the header's beginnings
	let header = ref_from_bytes_thiserror::<ChdHeaderBegin>(&buffer)?;
	if header.tag != *b"MComprHD" {
		return Err(ThisError::InvalidChdMagicNumber.into());
	}

	// access the SHA-1
	let sha1 = match u32::from(header.version) {
		3 => Ok(ref_from_bytes_thiserror::<ChdHeaderV3>(&buffer)?.sha1),
		4 => Ok(ref_from_bytes_thiserror::<ChdHeaderV4>(&buffer)?.sha1),
		5 => Ok(ref_from_bytes_thiserror::<ChdHeaderV5>(&buffer)?.sha1),
		_ => Err(ThisError::UnknownChdVersion),
	}?;

	// and return it!
	let sha1 = Some(sha1);
	Ok(AssetHash { crc: None, sha1 })
}

fn ref_from_bytes_thiserror<T>(source: &[u8]) -> Result<&T>
where
	T: FromBytes + KnownLayout + Immutable,
{
	let source = source.get(..size_of::<T>()).unwrap_or(source);
	T::ref_from_bytes(&source[..size_of::<T>()]).map_err(|_| ThisError::InvalidChdHeader.into())
}

#[cfg(test)]
mod test {
	use test_case::test_case;

	use crate::assethash::AssetHash;

	#[test_case(0, include_bytes!("test_data/samplechd.chd"), "bfec48ae2439308ac3a547231a13f122ef303c76")]
	fn chd_asset_hash(_index: usize, data: &[u8], expected: &str) {
		let expected = Ok(AssetHash::from_hex_strings(None, Some(expected)).unwrap());
		let actual = super::chd_asset_hash(data).map_err(|_| ());
		assert_eq!(expected, actual);
	}
}
