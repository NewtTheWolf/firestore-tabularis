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

/// Per-document, the optional reference target collection for any Reference-typed field.
/// Only populated for fields where classify_value() returned Reference.
pub type DocumentReferences = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub references: Option<String>,
}

/// Name of the synthetic primary-key column we expose for every collection,
/// carrying the Firestore document ID. If a real document field has the same
/// name, the synthetic column wins and the real field is hidden.
pub const ID_COLUMN: &str = "id";

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
            "is_pk": self.name == ID_COLUMN,
            "is_auto_increment": false,
            "character_maximum_length": Value::Null,
            "comment": if self.name == ID_COLUMN { Value::String("Firestore document ID".into()) } else { Value::Null },
            "references": self.references.as_ref().map(|s| Value::String(s.clone())).unwrap_or(Value::Null),
        })
    }
}

pub fn infer(sample: &[DocumentTypes], references: &[DocumentReferences]) -> Vec<ColumnInfo> {
    // Always-present synthetic ID column.
    let mut out = vec![ColumnInfo {
        name: ID_COLUMN.into(),
        data_type: "string".into(),
        is_nullable: false,
        references: None,
    }];

    // Collect, per field, the set of observed types.
    let mut types_by_field: BTreeMap<String, BTreeSet<FieldType>> = BTreeMap::new();
    let mut reference_targets_by_field: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for doc in sample {
        for (k, t) in doc {
            types_by_field.entry(k.clone()).or_default().insert(*t);
        }
    }

    for refs in references {
        for (k, target) in refs {
            reference_targets_by_field
                .entry(k.clone())
                .or_default()
                .insert(target.clone());
        }
    }

    for (name, types) in types_by_field {
        if name == ID_COLUMN {
            continue;
        }
        let (data_type, _has_null) = classify_set(&types);
        let references = reference_targets_by_field.get(&name).and_then(|targets| {
            if targets.len() == 1 {
                targets.iter().next().cloned()
            } else {
                None
            }
        });
        // Firestore is schemaless: any field may be absent in any document.
        // The sample-based "present in all sampled docs" signal is interesting
        // but does NOT amount to a required-ness constraint. Reporting
        // is_nullable=false here would make Tabularis block saves on fields
        // that Firestore would happily accept as missing — a UX bug rooted in
        // a relational-DB assumption that doesn't apply.
        out.push(ColumnInfo {
            name,
            data_type,
            is_nullable: true,
            references,
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

/// Walk one document and extract reference targets for any Reference-typed field.
/// The target is the collection segment immediately after `documents/` in the
/// reference's resource path.
pub fn references_from_document(doc: &firestore::FirestoreDocument) -> DocumentReferences {
    use gcloud_sdk::google::firestore::v1::value::ValueType as V;
    let mut out = DocumentReferences::new();
    for (name, val) in &doc.fields {
        if let Some(V::ReferenceValue(path)) = val.value_type.as_ref() {
            if let Some(target) = extract_target_collection(path) {
                out.insert(name.clone(), target);
            }
        }
    }
    out
}

fn extract_target_collection(resource_path: &str) -> Option<String> {
    // Find "documents/" then take the segment immediately after.
    let idx = resource_path.find("/documents/")?;
    let after = &resource_path[idx + "/documents/".len()..];
    after.split('/').next().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(pairs: &[(&str, FieldType)]) -> DocumentTypes {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn refs(pairs: &[(&str, &str)]) -> DocumentReferences {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn empty_sample_returns_only_id_column() {
        let cols = infer(&[], &[]);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].name, ID_COLUMN);
        assert_eq!(cols[0].data_type, "string");
        assert!(!cols[0].is_nullable);
    }

    #[test]
    fn single_doc_yields_id_plus_alphabetical_fields() {
        let sample = vec![doc(&[
            ("name", FieldType::String),
            ("age", FieldType::Integer),
        ])];
        let cols = infer(&sample, &[]);
        assert_eq!(
            cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["id", "age", "name"]
        );
        assert_eq!(cols[1].data_type, "number");
        assert_eq!(cols[2].data_type, "string");
        // All non-id fields are nullable in Firestore — no schema enforcement.
        assert!(cols[1].is_nullable);
    }

    #[test]
    fn integer_and_double_collapse_to_number() {
        let sample = vec![
            doc(&[("score", FieldType::Integer)]),
            doc(&[("score", FieldType::Double)]),
        ];
        let cols = infer(&sample, &[]);
        let score = cols.iter().find(|c| c.name == "score").unwrap();
        assert_eq!(score.data_type, "number");
    }

    #[test]
    fn null_co_observed_with_string_yields_nullable_string() {
        let sample = vec![
            doc(&[("note", FieldType::String)]),
            doc(&[("note", FieldType::Null)]),
        ];
        let cols = infer(&sample, &[]);
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
        let cols = infer(&sample, &[]);
        let flag = cols.iter().find(|c| c.name == "flag").unwrap();
        assert_eq!(flag.data_type, "mixed");
    }

    #[test]
    fn all_non_id_fields_report_nullable() {
        // Firestore is schemaless. Even a field present in 100% of sampled
        // docs may legitimately be omitted from a future doc — Tabularis must
        // not block save on its absence.
        let sample = vec![
            doc(&[("name", FieldType::String), ("nickname", FieldType::String)]),
            doc(&[("name", FieldType::String)]),
        ];
        let cols = infer(&sample, &[]);
        for col in &cols {
            if col.name == ID_COLUMN {
                continue;
            }
            assert!(col.is_nullable, "{} should be nullable", col.name);
        }
    }

    #[test]
    fn all_null_yields_null_data_type() {
        let sample = vec![doc(&[("placeholder", FieldType::Null)])];
        let cols = infer(&sample, &[]);
        let p = cols.iter().find(|c| c.name == "placeholder").unwrap();
        assert_eq!(p.data_type, "null");
        assert!(p.is_nullable);
    }

    #[test]
    fn nested_map_column_typed_as_map() {
        let sample = vec![doc(&[("address", FieldType::Map)])];
        let cols = infer(&sample, &[]);
        let a = cols.iter().find(|c| c.name == "address").unwrap();
        assert_eq!(a.data_type, "map");
    }

    #[test]
    fn id_column_serialises_as_primary_key() {
        let cols = infer(&[], &[]);
        let json = cols[0].to_json();
        assert_eq!(json["is_pk"], serde_json::Value::Bool(true));
        assert_eq!(
            json["comment"],
            serde_json::Value::String("Firestore document ID".into())
        );
    }

    #[test]
    fn reference_value_extracts_target_collection() {
        let sample = vec![doc(&[("author", FieldType::Reference)])];
        let refs_data = vec![refs(&[("author", "users")])];
        let cols = infer(&sample, &refs_data);
        let author = cols.iter().find(|c| c.name == "author").unwrap();
        assert_eq!(author.data_type, "reference");
        assert_eq!(author.references, Some("users".to_string()));
    }

    #[test]
    fn mixed_reference_targets_yield_no_fk() {
        let sample = vec![
            doc(&[("ref_field", FieldType::Reference)]),
            doc(&[("ref_field", FieldType::Reference)]),
        ];
        let refs_data = vec![
            refs(&[("ref_field", "users")]),
            refs(&[("ref_field", "advisors")]),
        ];
        let cols = infer(&sample, &refs_data);
        let f = cols.iter().find(|c| c.name == "ref_field").unwrap();
        assert_eq!(f.references, None);
    }

    #[test]
    fn no_reference_data_yields_no_fk() {
        let sample = vec![doc(&[("author", FieldType::Reference)])];
        let cols = infer(&sample, &[]);
        let f = cols.iter().find(|c| c.name == "author").unwrap();
        assert_eq!(f.references, None);
    }

    #[test]
    fn document_field_named_id_does_not_duplicate_synthetic_column() {
        // A document that legitimately stores a field called "id" (e.g. a
        // legacy import) must not produce two `id` columns. The synthetic
        // doc-ID column wins; the field-level `id` is dropped during inference.
        let sample = vec![doc(&[("id", FieldType::String), ("name", FieldType::String)])];
        let cols = infer(&sample, &[]);
        assert_eq!(
            cols.iter().filter(|c| c.name == ID_COLUMN).count(),
            1,
            "synthetic id column collapsed with field-level id"
        );
        let id_col = cols.iter().find(|c| c.name == ID_COLUMN).unwrap();
        // Synthetic id column has the doc-ID comment; field-level id wouldn't.
        let json = id_col.to_json();
        assert_eq!(json["is_pk"], serde_json::Value::Bool(true));
    }
}

#[cfg(test)]
mod resource_path_tests {
    use super::*;

    #[test]
    fn extracts_root_collection() {
        let path = "projects/p/databases/(default)/documents/users/abc123";
        assert_eq!(extract_target_collection(path), Some("users".to_string()));
    }

    #[test]
    fn handles_subcollection_doc() {
        let path = "projects/p/databases/(default)/documents/users/abc/orders/xyz";
        assert_eq!(extract_target_collection(path), Some("users".to_string()));
    }

    #[test]
    fn returns_none_for_unrecognised_path() {
        assert_eq!(extract_target_collection("garbage"), None);
    }
}
