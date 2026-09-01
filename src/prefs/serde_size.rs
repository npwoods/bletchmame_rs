use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use slint::PhysicalSize;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PhysicalSizeSerde<T> {
	width: T,
	height: T,
}

pub fn serialize<S>(value: &Option<PhysicalSize>, serializer: S) -> Result<S::Ok, S::Error>
where
	S: Serializer,
{
	match value {
		Some(value) => {
			let mapped = PhysicalSizeSerde::<u32> {
				width: value.width,
				height: value.height,
			};
			mapped.serialize(serializer)
		}
		None => serializer.serialize_none(),
	}
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<PhysicalSize>, D::Error>
where
	D: Deserializer<'de>,
{
	Option::<PhysicalSizeSerde<f64>>::deserialize(deserializer).map(|value| {
		value.map(|value| PhysicalSize {
			width: value.width as u32,
			height: value.height as u32,
		})
	})
}

#[cfg(test)]
mod test {
	use serde::Deserialize;
	use serde::Serialize;
	use serde_json::Value;
	use serde_json::json;
	use slint::PhysicalSize;
	use test_case::test_case;

	#[derive(Debug, Deserialize, Serialize, PartialEq)]
	struct Dummy {
		#[serde(flatten, with = "super")]
		size: Option<PhysicalSize>,
	}

	#[test_case(0, None, json!({}))]
	#[test_case(1, Some(PhysicalSize { width: 100, height: 100}), json!({"width": 100, "height": 100}))]
	#[test_case(2, Some(PhysicalSize { width: 250, height: 180}), json!({"width": 250, "height": 180}))]
	fn serialize(_index: usize, size: Option<PhysicalSize>, expected: Value) {
		let obj = Dummy { size };
		let actual = serde_json::to_value(&obj).unwrap();
		assert_eq!(expected, actual);
	}

	#[test_case(0, json!({}), None)]
	#[test_case(1, json!({"width": 100, "height": 100}), Some(PhysicalSize { width: 100, height: 100}))]
	#[test_case(2, json!({"width": 250, "height": 180}), Some(PhysicalSize { width: 250, height: 180}))]
	#[test_case(3, json!({"width": 100.0, "height": 100.0}), Some(PhysicalSize { width: 100, height: 100}))]
	#[test_case(4, json!({"width": 250.0, "height": 180.0}), Some(PhysicalSize { width: 250, height: 180}))]
	fn deserialize(_index: usize, json: Value, expected: Option<PhysicalSize>) {
		let actual = serde_json::from_value::<Dummy>(json).unwrap().size;
		assert_eq!(expected, actual);
	}
}
