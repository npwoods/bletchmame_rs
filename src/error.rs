use thiserror::Error;

pub type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Error, Debug)]
pub enum Error {
	#[error("Error processing MAME -listxml output at position {0}: {1}")]
	ListXmlProcessing(u64, BoxDynError),
	#[error("Error loading preferences: {0}")]
	PreferencesLoad(BoxDynError),
	#[error("Error saving preferences: {0}")]
	PreferencesSave(BoxDynError),
	#[error("Cannot find preferences directory")]
	CantFindPreferencesDirectory,
	#[error("Cannot determine InfoDB filename")]
	CannotBuildInfoDbFilename,
	#[error("Error loading software list: {0}")]
	SoftwareListLoad(BoxDynError),
	#[error("Error loading software list: No paths specified")]
	SoftwareListLoadNoPaths,
	#[error("Error parsing software list XML at position {0}: {1}")]
	SoftwareListXmlParsing(u64, BoxDynError),

	// InfoDB corruption
	#[error("Cannot deserialize InfoDB header")]
	CannotDeserializeInfoDbHeader,
	#[error("Bad InfoDB Magic Value In Header")]
	BadInfoDbMagicValue,
	#[error("Bad Sizes Hash In Header")]
	BadInfoDbSizesHash,
	#[error("Corrupt String Table")]
	CorruptStringTable,
}
