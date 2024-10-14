use thiserror::Error;

pub type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Error, Debug)]
pub enum Error {
	#[error("Error processing MAME -listxml output at position {0}: {1}")]
	ListXmlProcessing(u64, BoxDynError),
	#[error("Bad machine reference in MAME -listxml output")]
	BadMachineReference(String),
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
	#[error("Corrupt Software List Machine Index")]
	CorruptSoftwareListMachineIndex,

	// MAME preflights
	#[error("No path to MAME specified")]
	NoMamePathSpecified,
	#[error("Cannot find MAME")]
	CannotFindMame(BoxDynError),
	#[error("MAME is not a file")]
	MameIsNotAFile,

	// Bad MAME interactions conditions
	#[error("Error launching MAME: {0}")]
	MameLaunch(BoxDynError),
	#[error("Unexpected EOF from MAME: {0}")]
	EofFromMame(String),
	#[error("Error reading from MAME: {0}")]
	ReadingFromMame(BoxDynError),
	#[error("Error writing to MAME: {0}")]
	WritingToMame(BoxDynError),
	#[error("MAME Error Response: {0}")]
	MameErrorResponse(String),
	#[error("Response not understood: {0}")]
	MameResponseNotUnderstood(String),
	#[error("Error parsing status XML at position {0}: {1}")]
	StatusXmlProcessing(u64, BoxDynError),
}
