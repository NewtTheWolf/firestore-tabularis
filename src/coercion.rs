//! Edit-cell value (JSON) → Firestore proto value.
//!
//! The inverse of `handlers::query::serialize_value`. Tabularis sends edited
//! cell values as plain JSON; we need to materialise them as the right proto
//! variant so the document round-trips correctly.
//!
//! When a column has a known type from `SCHEMA_CACHE` we use it as a hint:
//! a string typed `timestamp` is parsed as RFC3339; `array`/`map` strings get
//! JSON-parsed because Phase 2 ships nested values as JSON-stringified blobs
//! (Tabularis grid limitation — see ROADMAP Phase 4 for the inverse).
//!
//! Without a hint, fall back to the JSON shape: number → integer/double, bool
//! → bool, null → null, string → string, array → array, object → map.

use base64::Engine;
use gcloud_sdk::google::firestore::v1::value::ValueType;
use gcloud_sdk::google::firestore::v1::{ArrayValue, MapValue, Value as ProtoValue};
use serde_json::Value;

/// Coerce a serde_json `Value` into a Firestore proto `Value` using an
/// optional type hint from the inferred schema.
///
/// The hint is **advisory**: if it disagrees with the JSON shape (e.g. hint
/// "number" but JSON is a string), we fall back to the JSON shape rather than
/// rejecting the input. Strict validation is the caller's job — at this layer
/// we want lossless round-trips for as many real-world payloads as possible.
pub fn json_to_proto(value: &Value, hint: Option<&str>) -> ProtoValue {
    let value_type = Some(match value {
        Value::Null => ValueType::NullValue(0),
        Value::Bool(b) => ValueType::BooleanValue(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ValueType::IntegerValue(i)
            } else if let Some(f) = n.as_f64() {
                ValueType::DoubleValue(f)
            } else {
                // Out-of-range integer (u64 ≥ 2^63) → fall back to string.
                ValueType::StringValue(n.to_string())
            }
        }
        Value::String(s) => coerce_string(s, hint),
        Value::Array(items) => ValueType::ArrayValue(ArrayValue {
            values: items.iter().map(|v| json_to_proto(v, None)).collect(),
        }),
        Value::Object(map) => ValueType::MapValue(MapValue {
            fields: map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_proto(v, None)))
                .collect(),
        }),
    });
    ProtoValue { value_type }
}

fn coerce_string(s: &str, hint: Option<&str>) -> ValueType {
    match hint {
        Some("timestamp") => parse_timestamp(s).unwrap_or_else(|| ValueType::StringValue(s.into())),
        Some("reference") => ValueType::ReferenceValue(s.into()),
        Some("binary") => match base64::engine::general_purpose::STANDARD.decode(s) {
            Ok(bytes) => ValueType::BytesValue(bytes),
            Err(_) => ValueType::StringValue(s.into()),
        },
        // Phase 2 ships array/map cells as JSON-stringified blobs. When the
        // user edits them, we get the same shape back — try parsing, fall back
        // to plain string if it's no longer valid JSON.
        Some("array") | Some("map") => match serde_json::from_str::<Value>(s) {
            Ok(parsed) => json_to_proto(&parsed, None).value_type.unwrap(),
            Err(_) => ValueType::StringValue(s.into()),
        },
        _ => ValueType::StringValue(s.into()),
    }
}

fn parse_timestamp(s: &str) -> Option<ValueType> {
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    let utc = dt.with_timezone(&chrono::Utc);
    Some(ValueType::TimestampValue(
        gcloud_sdk::prost_types::Timestamp {
            seconds: utc.timestamp(),
            nanos: utc.timestamp_subsec_nanos() as i32,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vt(v: &ProtoValue) -> &ValueType {
        v.value_type.as_ref().unwrap()
    }

    #[test]
    fn null_round_trips() {
        let v = json_to_proto(&Value::Null, None);
        assert!(matches!(vt(&v), ValueType::NullValue(_)));
    }

    #[test]
    fn bool_round_trips() {
        let v = json_to_proto(&json!(true), None);
        assert!(matches!(vt(&v), ValueType::BooleanValue(true)));
    }

    #[test]
    fn integer_becomes_integer_value() {
        let v = json_to_proto(&json!(42), None);
        assert!(matches!(vt(&v), ValueType::IntegerValue(42)));
    }

    #[test]
    fn float_becomes_double_value() {
        let v = json_to_proto(&json!(2.5), None);
        match vt(&v) {
            ValueType::DoubleValue(f) => assert!((f - 2.5).abs() < f64::EPSILON),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn plain_string_with_no_hint_stays_string() {
        let v = json_to_proto(&json!("hello"), None);
        assert!(matches!(vt(&v), ValueType::StringValue(s) if s == "hello"));
    }

    #[test]
    fn timestamp_hint_parses_rfc3339() {
        let v = json_to_proto(&json!("2026-05-09T10:30:00Z"), Some("timestamp"));
        match vt(&v) {
            ValueType::TimestampValue(t) => {
                // 2026-05-09T10:30:00Z = 1778322600
                assert_eq!(t.seconds, 1778322600);
                assert_eq!(t.nanos, 0);
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn invalid_timestamp_falls_back_to_string() {
        let v = json_to_proto(&json!("not a date"), Some("timestamp"));
        assert!(matches!(vt(&v), ValueType::StringValue(_)));
    }

    #[test]
    fn reference_hint_emits_reference_value() {
        let v = json_to_proto(
            &json!("projects/p/databases/(default)/documents/users/abc"),
            Some("reference"),
        );
        assert!(matches!(vt(&v), ValueType::ReferenceValue(_)));
    }

    #[test]
    fn array_hint_parses_json_string() {
        let v = json_to_proto(&json!("[\"a\", \"b\"]"), Some("array"));
        match vt(&v) {
            ValueType::ArrayValue(a) => assert_eq!(a.values.len(), 2),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn map_hint_parses_json_string() {
        let v = json_to_proto(&json!("{\"k\": 1}"), Some("map"));
        match vt(&v) {
            ValueType::MapValue(m) => {
                let entry = m.fields.get("k").unwrap();
                assert!(matches!(vt(entry), ValueType::IntegerValue(1)));
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn malformed_array_hint_falls_back_to_string() {
        let v = json_to_proto(&json!("[invalid"), Some("array"));
        assert!(matches!(vt(&v), ValueType::StringValue(_)));
    }

    #[test]
    fn nested_object_becomes_map() {
        let v = json_to_proto(&json!({"a": 1, "b": "x"}), None);
        match vt(&v) {
            ValueType::MapValue(m) => {
                assert_eq!(m.fields.len(), 2);
                assert!(matches!(vt(m.fields.get("a").unwrap()), ValueType::IntegerValue(1)));
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn binary_hint_decodes_base64() {
        let v = json_to_proto(&json!("aGVsbG8="), Some("binary")); // "hello"
        match vt(&v) {
            ValueType::BytesValue(b) => assert_eq!(b, b"hello"),
            other => panic!("{:?}", other),
        }
    }
}
