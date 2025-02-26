use serde::Deserializer;
use serde::Serializer;
use serde::de::MapAccess;
use serde::de::Visitor;
use serde::ser::SerializeMap;

#[derive(Debug, Default)]
struct SlotsVisitor();

impl<'de> Visitor<'de> for SlotsVisitor {
	type Value = Vec<(String, Option<String>)>;

	fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(formatter, "a slot map")
	}

	fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
	where
		M: MapAccess<'de>,
	{
		let mut result = Self::Value::default();
		while let Some((key, value)) = access.next_entry()? {
			result.push((key, value));
		}
		result.sort_by(|(a, _), (b, _)| Ord::cmp(a, b));
		Ok(result)
	}
}

pub fn serialize<S>(slots: &[(String, Option<String>)], serializer: S) -> Result<S::Ok, S::Error>
where
	S: Serializer,
{
	let mut map = serializer.serialize_map(Some(slots.len()))?;
	for (key, value) in slots {
		map.serialize_entry(key, value)?;
	}
	map.end()
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<(String, Option<String>)>, D::Error>
where
	D: Deserializer<'de>,
{
	let visitor = SlotsVisitor::default();
	deserializer.deserialize_map(visitor)
}

#[cfg(test)]
mod test {
	use itertools::Itertools;
	use serde::Deserialize;
	use serde::Serialize;
	use test_case::test_case;

	#[derive(Deserialize, Serialize)]
	struct Dummy {
		#[serde(with = "super")]
		pub field: Vec<(String, Option<String>)>,
	}

	#[test_case(0, r#"{}"#, &[])]
	#[test_case(1, r#"{ "ext": null }"#, &[("ext", None)])]
	#[test_case(2, r#"{ "ext": "multi" }"#, &[("ext", Some("multi"))])]
	#[test_case(3, r#"{ "ext": "multi", "ext:multi:slot4:fdc:wd17xx:1": "525sd" }"#, &[("ext", Some("multi")), ("ext:multi:slot4:fdc:wd17xx:1", Some("525sd"))])]
	pub fn test(_index: usize, json: &str, expected: &[(&str, Option<&str>)]) {
		let json = format!(r#"{{ "field": {json} }}"#);
		let obj = serde_json::from_str::<Dummy>(&json).unwrap();
		let actual = obj.field.as_slice();

		let expected = expected
			.iter()
			.map(|(key, value)| (key.to_string(), value.map(str::to_string)))
			.collect::<Vec<_>>();
		assert_eq!(expected.as_slice(), actual);

		let new_json = serde_json::to_string_pretty(&obj).unwrap().split_whitespace().join(" ");
		assert_eq!(&json, &new_json);
	}
}
