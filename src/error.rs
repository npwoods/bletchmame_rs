use thiserror::Error;

pub type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Error, Debug)]
pub enum Error {
	#[error("Error processing MAME -listxml output at position {0}: {1}")]
	ListXmlProcessing(u64, BoxDynError),
	#[error("Bad machine reference in MAME -listxml output")]
	BadMachineReference(String),
	#[error("Error loading preferences: {0}")]
	PreferencesLoadIo(std::io::Error),
	#[error("Error loading preferences: {0}")]
	PreferencesLoadDeserl(BoxDynError),
	#[error("Error saving preferences: {0}")]
	PreferencesSave(BoxDynError),
	#[error("Cannot find preferences directory")]
	CantFindPreferencesDirectory,
	#[error("Cannot determine InfoDB filename")]
	CannotBuildInfoDbFilename,
	#[error("Error loading InfoDB: {0}")]
	InfoDbLoad(BoxDynError),
	#[error("Error saving InfoDB: {0}")]
	InfoDbSave(BoxDynError),
	#[error("Error loading software list: {0}")]
	SoftwareListLoad(BoxDynError),
	#[error("Error loading software list: No paths specified")]
	SoftwareListLoadNoPaths,
	#[error("Error parsing software list XML at position {0}: {1}")]
	SoftwareListXmlParsing(u64, BoxDynError),
	#[error("Problems found during MAME preflight: {0:?}")]
	MamePreflightProblems(Vec<PreflightProblem>),

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

#[derive(Copy, Clone, Debug, strum_macros::Display)]
pub enum PreflightProblem {
	#[strum(to_string = "No MAME executable path specified")]
	NoMameExecutablePath,
	#[strum(to_string = "No MAME executable found")]
	NoMameExecutable,
	#[strum(to_string = "MAME executable file is not executable")]
	MameExecutableIsNotExecutable,
	#[strum(to_string = "No valid plugins paths specified")]
	NoPluginsPaths,
	#[strum(to_string = "MAME boot.lua not found")]
	PluginsBootNotFound,
	#[strum(to_string = "BletchMAME worker_ui plugin not found")]
	WorkerUiPluginNotFound,
}
