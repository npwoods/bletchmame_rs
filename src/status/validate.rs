use crate::info::InfoDb;
use crate::status::Running;
use crate::status::Status;
use crate::status::UpdateXmlProblem;
use crate::status::ValidationError;

pub fn validate_status(status: &Status, info_db: &InfoDb) -> Result<(), ValidationError> {
	// first order of business is to check for a version mismatch
	if status.build != *info_db.build() {
		let mame_build = status.build.clone();
		let infodb_build = info_db.build().clone();
		Err(ValidationError::VersionMismatch(mame_build, infodb_build))
	} else {
		// with that out of the way, check for specific problems
		let mut problems = Vec::new();
		if let Some(running) = &status.running {
			validate_running(running, info_db, |x| problems.push(x));
		}

		// if we found no problems, we've succeeded; otherwise error
		if problems.is_empty() {
			Ok(())
		} else {
			Err(ValidationError::Invalid(problems))
		}
	}
}

fn validate_running(running: &Running, info_db: &InfoDb, mut emit: impl FnMut(UpdateXmlProblem)) {
	if info_db.machines().find(&running.machine_name).is_err() {
		emit(UpdateXmlProblem::UnknownMachine(running.machine_name.clone()));
	}
}

#[cfg(test)]
mod test {
	use std::io::BufReader;

	use test_case::test_case;

	use crate::info::InfoDb;
	use crate::status::Status;
	use crate::status::Update;
	use crate::status::UpdateXmlProblem::UnknownMachine;
	use crate::status::ValidationError;
	use crate::status::ValidationError::Invalid;
	use crate::status::ValidationError::VersionMismatch;
	use crate::version::MameVersion;

	fn vers(s: &str) -> MameVersion {
		MameVersion::parse_simple(s).unwrap()
	}

	#[test_case(0, include_str!("../info/test_data/listxml_c64.xml"), include_str!("test_data/status_mame0273_c64_1.xml"), Ok(()))]
	#[test_case(1, include_str!("../info/test_data/listxml_alienar.xml"), include_str!("test_data/status_mame0273_c64_1.xml"), Err(VersionMismatch(vers("0.273"), vers("0.229"))))]
	#[test_case(2, include_str!("../info/test_data/listxml_c64.xml"), include_str!("test_data/status_mame0273_alienar_1.xml"), Err(Invalid(vec![UnknownMachine("alienar".into())])))]
	pub fn test(_index: usize, info_xml: &str, update_xml: &str, expected: Result<(), ValidationError>) {
		let info_db = InfoDb::from_listxml_output(info_xml.as_bytes(), |_| false)
			.unwrap()
			.unwrap();
		let update_reader = BufReader::new(update_xml.as_bytes());
		let update = Update::parse(update_reader).unwrap();
		let status = Status::new(None, update);
		let actual = status.validate(&info_db);
		assert_eq!(expected, actual);
	}
}
