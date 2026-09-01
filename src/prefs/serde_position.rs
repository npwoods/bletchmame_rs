use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use slint::PhysicalPosition;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PhysicalPositionSerde<T> {
	x: T,
	y: T,
}

pub fn serialize<S>(value: &Option<PhysicalPosition>, serializer: S) -> Result<S::Ok, S::Error>
where
	S: Serializer,
{
	match value {
		Some(value) => {
			let mapped = PhysicalPositionSerde::<i32> { x: value.x, y: value.y };
			mapped.serialize(serializer)
		}
		None => serializer.serialize_none(),
	}
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<PhysicalPosition>, D::Error>
where
	D: Deserializer<'de>,
{
	Option::<PhysicalPositionSerde<f64>>::deserialize(deserializer).map(|value| {
		value.map(|position| PhysicalPosition {
			x: position.x as i32,
			y: position.y as i32,
		})
	})
}

#[cfg(test)]
mod test {
	use serde::Deserialize;
	use serde::Serialize;
	use serde_json::Value;
	use serde_json::json;
	use slint::PhysicalPosition;
	use test_case::test_case;

	#[derive(Debug, Deserialize, Serialize, PartialEq)]
	struct Dummy {
		#[serde(flatten, with = "super")]
		position: Option<PhysicalPosition>,
	}

	#[test_case(0, None, json!({}))]
	#[test_case(1, Some(PhysicalPosition { x: 100, y: 200 }), json!({"x": 100, "y": 200}))]
	#[test_case(2, Some(PhysicalPosition { x: -25, y: 180 }), json!({"x": -25, "y": 180}))]
	fn serialize(_index: usize, position: Option<PhysicalPosition>, expected: Value) {
		let obj = Dummy { position };
		let actual = serde_json::to_value(&obj).unwrap();
		assert_eq!(expected, actual);
	}

	#[test_case(0, json!({}), None)]
	#[test_case(1, json!({"x": 100, "y": 200}), Some(PhysicalPosition { x: 100, y: 200 }))]
	#[test_case(2, json!({"x": -25, "y": 180}), Some(PhysicalPosition { x: -25, y: 180 }))]
	#[test_case(3, json!({"x": 100.0, "y": 200.0}), Some(PhysicalPosition { x: 100, y: 200 }))]
	#[test_case(4, json!({"x": -25.0, "y": 180.0}), Some(PhysicalPosition { x: -25, y: 180 }))]
	fn deserialize(_index: usize, json: Value, expected: Option<PhysicalPosition>) {
		let actual = serde_json::from_value::<Dummy>(json).unwrap().position;
		assert_eq!(expected, actual);
	}
}
