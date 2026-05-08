//! Sample-based column inference.
//!
//! The classifier (`FieldType`) and the algorithm (`infer`) are decoupled
//! from `firestore-rs`. The handler in `handlers/metadata.rs` is responsible
//! for converting a real `FirestoreDocument` into a `Vec<DocumentTypes>`
//! before calling `infer`.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldType {
    String,
    Integer,
    Double,
    Boolean,
    Timestamp,
    Bytes,
    GeoPoint,
    Reference,
    Array,
    Map,
    Null,
}

/// Map of field-name → set of Firestore types observed for that field within a single document.
/// (A single field within a single document has exactly one type, so the inner Set is conceptual:
/// we use it to fold across all sample docs in `infer`.)
pub type DocumentTypes = BTreeMap<String, FieldType>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
}

impl ColumnInfo {
    pub fn to_json(&self) -> Value {
        // Field names match the working tabularis-google-sheets-plugin reference
        // implementation, not the published PLUGIN_GUIDE.md (which is stale on
        // these names). Tabularis runtime parses `default_value` and `is_pk`,
        // NOT `column_default`/`is_primary_key`.
        json!({
            "name": self.name,
            "data_type": self.data_type,
            "is_nullable": self.is_nullable,
            "default_value": Value::Null,
            "is_pk": self.name == "__id__",
            "is_auto_increment": false,
            "character_maximum_length": Value::Null,
            "comment": if self.name == "__id__" { Value::String("Firestore document ID".into()) } else { Value::Null },
        })
    }
}

pub fn infer(sample: &[DocumentTypes]) -> Vec<ColumnInfo> {
    // Always-present synthetic ID column.
    let mut out = vec![ColumnInfo {
        name: "__id__".into(),
        data_type: "string".into(),
        is_nullable: false,
    }];

    // Collect, per field, the set of observed types.
    let mut types_by_field: BTreeMap<String, BTreeSet<FieldType>> = BTreeMap::new();

    let total = sample.len();
    let mut seen_count: BTreeMap<String, usize> = BTreeMap::new();

    for doc in sample {
        for (k, t) in doc {
            types_by_field.entry(k.clone()).or_default().insert(*t);
            *seen_count.entry(k.clone()).or_insert(0) += 1;
        }
    }

    for (name, types) in types_by_field {
        let (data_type, has_null) = classify_set(&types);
        let missing = seen_count.get(&name).is_none_or(|&c| c < total);
        let is_nullable = has_null || missing;
        out.push(ColumnInfo {
            name,
            data_type,
            is_nullable,
        });
    }

    out
}

fn classify_set(types: &BTreeSet<FieldType>) -> (String, bool) {
    let has_null = types.contains(&FieldType::Null);
    // BTreeSet iterates in sorted order, so non_null is already sorted (relied on by the [Integer, Double] match arm).
    let non_null: Vec<FieldType> = types
        .iter()
        .copied()
        .filter(|t| *t != FieldType::Null)
        .collect();

    let data_type = match non_null.as_slice() {
        [] => "null",
        [FieldType::String] => "string",
        [FieldType::Integer] | [FieldType::Double] | [FieldType::Integer, FieldType::Double] => {
            "number"
        }
        [FieldType::Boolean] => "boolean",
        [FieldType::Timestamp] => "timestamp",
        [FieldType::Bytes] => "binary",
        [FieldType::GeoPoint] => "geopoint",
        [FieldType::Reference] => "reference",
        [FieldType::Array] => "array",
        [FieldType::Map] => "map",
        _ => "mixed",
    };

    (data_type.to_string(), has_null)
}

/// Convert a single Firestore field value into our coarse `FieldType`.
pub fn classify_value(v: &gcloud_sdk::google::firestore::v1::Value) -> FieldType {
    use gcloud_sdk::google::firestore::v1::value::ValueType as V;

    match v.value_type.as_ref() {
        Some(V::NullValue(_)) => FieldType::Null,
        Some(V::BooleanValue(_)) => FieldType::Boolean,
        Some(V::IntegerValue(_)) => FieldType::Integer,
        Some(V::DoubleValue(_)) => FieldType::Double,
        Some(V::TimestampValue(_)) => FieldType::Timestamp,
        Some(V::StringValue(_)) => FieldType::String,
        Some(V::BytesValue(_)) => FieldType::Bytes,
        Some(V::ReferenceValue(_)) => FieldType::Reference,
        Some(V::GeoPointValue(_)) => FieldType::GeoPoint,
        Some(V::ArrayValue(_)) => FieldType::Array,
        Some(V::MapValue(_)) => FieldType::Map,
        // Extra variants present in the proto but not standard storage types:
        Some(V::FieldReferenceValue(_)) => FieldType::Reference,
        Some(V::FunctionValue(_)) => FieldType::Null,
        Some(V::PipelineValue(_)) => FieldType::Null,
        None => FieldType::Null,
    }
}

/// Walk one document's top-level fields and collapse them into a `DocumentTypes` map.
pub fn types_from_document(doc: &firestore::FirestoreDocument) -> DocumentTypes {
    doc.fields
        .iter()
        .map(|(name, val)| (name.clone(), classify_value(val)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(pairs: &[(&str, FieldType)]) -> DocumentTypes {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn empty_sample_returns_only_id_column() {
        let cols = infer(&[]);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].name, "__id__");
        assert_eq!(cols[0].data_type, "string");
        assert!(!cols[0].is_nullable);
    }

    #[test]
    fn single_doc_yields_id_plus_alphabetical_fields() {
        let sample = vec![doc(&[
            ("name", FieldType::String),
            ("age", FieldType::Integer),
        ])];
        let cols = infer(&sample);
        assert_eq!(
            cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["__id__", "age", "name"]
        );
        assert_eq!(cols[1].data_type, "number");
        assert_eq!(cols[2].data_type, "string");
        assert!(!cols[1].is_nullable);
    }

    #[test]
    fn integer_and_double_collapse_to_number() {
        let sample = vec![
            doc(&[("score", FieldType::Integer)]),
            doc(&[("score", FieldType::Double)]),
        ];
        let cols = infer(&sample);
        let score = cols.iter().find(|c| c.name == "score").unwrap();
        assert_eq!(score.data_type, "number");
    }

    #[test]
    fn null_co_observed_with_string_yields_nullable_string() {
        let sample = vec![
            doc(&[("note", FieldType::String)]),
            doc(&[("note", FieldType::Null)]),
        ];
        let cols = infer(&sample);
        let note = cols.iter().find(|c| c.name == "note").unwrap();
        assert_eq!(note.data_type, "string");
        assert!(note.is_nullable);
    }

    #[test]
    fn conflicting_types_yield_mixed() {
        let sample = vec![
            doc(&[("flag", FieldType::Boolean)]),
            doc(&[("flag", FieldType::String)]),
        ];
        let cols = infer(&sample);
        let flag = cols.iter().find(|c| c.name == "flag").unwrap();
        assert_eq!(flag.data_type, "mixed");
    }

    #[test]
    fn missing_field_in_some_docs_marks_nullable() {
        let sample = vec![
            doc(&[("name", FieldType::String), ("nickname", FieldType::String)]),
            doc(&[("name", FieldType::String)]),
        ];
        let cols = infer(&sample);
        let name = cols.iter().find(|c| c.name == "name").unwrap();
        let nickname = cols.iter().find(|c| c.name == "nickname").unwrap();
        assert!(!name.is_nullable);
        assert!(nickname.is_nullable);
    }

    #[test]
    fn all_null_yields_null_data_type() {
        let sample = vec![doc(&[("placeholder", FieldType::Null)])];
        let cols = infer(&sample);
        let p = cols.iter().find(|c| c.name == "placeholder").unwrap();
        assert_eq!(p.data_type, "null");
        assert!(p.is_nullable);
    }

    #[test]
    fn nested_map_column_typed_as_map() {
        let sample = vec![doc(&[("address", FieldType::Map)])];
        let cols = infer(&sample);
        let a = cols.iter().find(|c| c.name == "address").unwrap();
        assert_eq!(a.data_type, "map");
    }

    #[test]
    fn id_column_serialises_as_primary_key() {
        let cols = infer(&[]);
        let json = cols[0].to_json();
        assert_eq!(json["is_pk"], serde_json::Value::Bool(true));
        assert_eq!(
            json["comment"],
            serde_json::Value::String("Firestore document ID".into())
        );
    }
}
