use std::fs::File;
use std::io::Cursor;
use std::io::Read;
use std::io::Seek;
use std::path::Path;

use anyhow::Result;
use itertools::Either;
use sevenz_rust2::ArchiveReader as SevenZArchiveReader;
use strum::VariantArray;
use zip::ZipArchive;
use zip::read::ZipFile;
use zip::result::ZipError;

/// Identifies a supported archive file format
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, VariantArray)]
pub enum ArchiveFormat {
	Zip,
	SevenZ,
}

impl ArchiveFormat {
	/// The canonical file extension for this format
	pub fn extension(&self) -> &'static str {
		match self {
			Self::Zip => "zip",
			Self::SevenZ => "7z",
		}
	}
}

#[derive(Debug, thiserror::Error)]
enum ThisError {
	#[error("Unknown archive file extension: {0}")]
	UnknownArchiveFileExtension(String),
}

/// A read-only archive file in one of the supported formats (see `ArchiveFormat`)
pub struct Archive<R: Read + Seek = File> {
	inner: ArchiveInner<R>,
}

enum ArchiveInner<R: Read + Seek> {
	Zip(ZipArchive<R>),
	SevenZ(Box<SevenZArchiveReader<R>>),
}

impl Archive<File> {
	/// Opens an archive file, dispatching on the path's file extension
	pub fn open(path: &Path) -> Result<Self> {
		let extension = path.extension().and_then(|x| x.to_str()).map(|x| x.to_lowercase());
		let format = ArchiveFormat::VARIANTS
			.iter()
			.find(|format| Some(format.extension()) == extension.as_deref())
			.copied()
			.ok_or_else(|| ThisError::UnknownArchiveFileExtension(extension.unwrap_or_default()))?;
		Self::new(File::open(path)?, format)
	}
}

impl<R> Archive<R>
where
	R: Read + Seek,
{
	/// Opens an archive in the specified format from an already open reader
	pub fn new(reader: R, format: ArchiveFormat) -> Result<Self> {
		let inner = match format {
			ArchiveFormat::Zip => ArchiveInner::Zip(ZipArchive::new(reader)?),
			ArchiveFormat::SevenZ => {
				ArchiveInner::SevenZ(Box::new(SevenZArchiveReader::new(reader, Default::default())?))
			}
		};
		Ok(Self { inner })
	}

	/// Lists the names of all file entries in the archive
	pub fn list(&self) -> impl Iterator<Item = &'_ str> {
		match &self.inner {
			ArchiveInner::Zip(zip) => {
				let iter = zip.file_names().filter(|name| !name.ends_with('/'));
				Either::Left(iter)
			}
			ArchiveInner::SevenZ(sevenz) => {
				let iter = sevenz
					.archive()
					.files
					.iter()
					.filter(|entry| !entry.is_directory)
					.map(|entry| entry.name.as_str());
				Either::Right(iter)
			}
		}
	}

	/// Reads the entire entry with the given name; returns `Ok(None)` if no such entry exists
	pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
		match &mut self.inner {
			ArchiveInner::Zip(zip) => match zip.by_name(name) {
				Ok(mut file) => {
					let mut buf = Vec::new();
					file.read_to_end(&mut buf)?;
					Ok(Some(buf))
				}
				Err(ZipError::FileNotFound) => Ok(None),
				Err(e) => Err(e.into()),
			},
			ArchiveInner::SevenZ(sevenz) => match sevenz.read_file(name) {
				Ok(data) => Ok(Some(data)),
				Err(sevenz_rust2::Error::FileNotFound) => Ok(None),
				Err(e) => Err(e.into()),
			},
		}
	}

	/// Reads the entry with the given name, falling back to searching by CRC if the name is not
	/// found; this emulates MAME's behavior by which you can load assets by CRC even if the name
	/// is wrong
	pub fn read_by_name_or_crc(&mut self, name: &str, crc: Option<u32>) -> Result<Entry<'_, R>> {
		let inner = match &mut self.inner {
			ArchiveInner::Zip(zip) => {
				let file_number = match (zip.index_for_name(name), crc) {
					(Some(index), _) => Some(index),
					(None, None) => None,
					(None, Some(crc)) => (0..zip.len())
						.find(|&file_number| zip.by_index(file_number).is_ok_and(|file| file.crc32() == crc)),
				};
				let file = file_number
					.map(|file_number| zip.by_index(file_number))
					.unwrap_or(Err(ZipError::FileNotFound))?;
				EntryInner::Zip(Box::new(file))
			}
			ArchiveInner::SevenZ(sevenz) => {
				let data = match sevenz.read_file(name) {
					Ok(data) => Ok(data),
					Err(e) => {
						let by_crc = crc.and_then(|crc| {
							sevenz
								.archive()
								.files
								.iter()
								.find(|f| f.has_stream && f.has_crc && f.crc == crc as u64)
								.map(|f| f.name.clone())
								.and_then(|entry_name| sevenz.read_file(&entry_name).ok())
						});
						by_crc.ok_or(e)
					}
				}?;
				EntryInner::SevenZ(Cursor::new(data))
			}
		};
		let size = match &inner {
			EntryInner::Zip(file) => file.size(),
			EntryInner::SevenZ(data) => data.get_ref().len() as u64,
		};
		Ok(Entry { inner, size })
	}
}

/// A single entry read from an `Archive`, which can be streamed using the `Read` trait
pub struct Entry<'a, R: Read + Seek = File> {
	size: u64,
	inner: EntryInner<'a, R>,
}

enum EntryInner<'a, R: Read + Seek> {
	Zip(Box<ZipFile<'a, R>>),
	SevenZ(Cursor<Vec<u8>>),
}

impl<R: Read + Seek> Entry<'_, R> {
	/// The size of this entry, in bytes
	pub fn size(&self) -> u64 {
		self.size
	}
}

impl<R: Read + Seek> Read for Entry<'_, R> {
	fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
		match &mut self.inner {
			EntryInner::Zip(file) => file.read(buf),
			EntryInner::SevenZ(cursor) => cursor.read(buf),
		}
	}
}

#[cfg(test)]
mod tests {
	use std::io::Cursor;
	use std::io::Read;

	use test_case::test_case;

	use super::Archive;
	use super::ArchiveFormat;

	#[test_case(0, "correct_name.bin", None, Some(0x3b5b2bc1))]
	#[test_case(1, "nonexistent_name.bin", Some(0x098825db), Some(0x098825db))]
	#[test_case(2, "correct_name.bin", Some(0x098825db), Some(0x3b5b2bc1))]
	#[test_case(3, "nonexistent.bin", None, None)]
	#[test_case(4, "nonexistent.bin", Some(0xdeadbeef), None)]
	fn read_by_name_or_crc(_index: usize, name: &str, crc: Option<u32>, expected: Option<u32>) {
		// helper to read an entry to completion and identify it by CRC
		fn get_crc<R: std::io::Read + std::io::Seek>(
			archive: &mut Archive<R>,
			name: &str,
			crc: Option<u32>,
		) -> Option<u32> {
			let mut entry = archive.read_by_name_or_crc(name, crc).ok()?;
			let mut buf = Vec::new();
			entry.read_to_end(&mut buf).unwrap();
			Some(crc32fast::hash(&buf))
		}

		// validate the ZIP version
		let zip_bytes = include_bytes!("test_data/ziparchive01.zip");
		let mut archive = Archive::new(Cursor::new(zip_bytes), ArchiveFormat::Zip).unwrap();
		assert_eq!(expected, get_crc(&mut archive, name, crc));

		// and the 7z version
		let sevenz_bytes = include_bytes!("test_data/7zarchive01.7z");
		let mut archive = Archive::new(Cursor::new(sevenz_bytes), ArchiveFormat::SevenZ).unwrap();
		assert_eq!(expected, get_crc(&mut archive, name, crc));
	}

	#[test_case(0, include_bytes!("test_data/ziparchive01.zip"), ArchiveFormat::Zip, &["correct_name.bin", "wrong_name.bin"])]
	#[test_case(1, include_bytes!("test_data/7zarchive01.7z"), ArchiveFormat::SevenZ, &["correct_name.bin", "wrong_name.bin"])]
	fn list_filenames(_index: usize, bytes: &[u8], format: ArchiveFormat, expected: &[&str]) {
		let cursor = Cursor::new(bytes);
		let archive = Archive::new(cursor, format).unwrap();
		let actual = archive.list().map(|x| x.to_string()).collect::<Vec<_>>();
		let actual = actual.iter().map(|s| s.as_str()).collect::<Vec<_>>();
		assert_eq!(expected, actual);
	}
}
