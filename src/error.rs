use thiserror::Error;

type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

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
