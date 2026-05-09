//! Per-collection schema overrides for power users.
//!
//! Firestore is schemaless, so `schema_infer` reports a permissive shape:
//! every non-`id` field as nullable, type derived from the sample. This is
//! correct for the storage engine but at odds with how teams actually use
//! Firestore — most collections have a *de facto* schema enforced by
//! application code (or Firestore Security Rules).
//!
//! This module loads a user-supplied JSON file that lets power users
//! declare:
//!   - `required: true` — overrides our nullable=true default so Tabularis
//!     blocks save when the field is empty.
//!   - `type: "..."` — corrects sample-driven inference, e.g. when the same
//!     field stores both integer and double values and we report "mixed".
//!   - `hidden: true` — drops the column from the grid entirely.
//!   - `comment: "..."` — surfaces in the column tooltip / ER diagram.
//!   - extra fields not in the sample (rare-but-valid fields).
//!
//! The file is loaded once at `initialize`. Edit, then toggle the connection
//! in Tabularis to pick up changes — file-watching is intentionally not
//! implemented (too many edge cases for too little benefit).
//!
//! # Example
//! ```json
//! {
//!   "collections": {
//!     "advisors": {
//!       "fields": {
//!         "email":         { "required": true, "comment": "Login identifier" },
//!         "rating":        { "type": "number", "required": false },
//!         "internalNotes": { "hidden": true }
//!       },
//!       "extra_fields": {
//!         "rarelyUsedField": { "type": "string", "required": false }
//!       }
//!     }
//!   }
//! }
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::schema_infer::ColumnInfo;

#[derive(Debug, Default, Clone, Deserialize)]
pub struct SchemaOverrides {
    #[serde(default)]
    pub collections: HashMap<String, CollectionOverride>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct CollectionOverride {
    #[serde(default)]
    pub fields: HashMap<String, FieldOverride>,
    /// Fields not present in the sample but that should appear in the grid.
    #[serde(default)]
    pub extra_fields: HashMap<String, FieldOverride>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct FieldOverride {
    /// `Some(true)` flips `is_nullable` to false; `Some(false)` keeps it
    /// nullable. `None` leaves the inference default in place.
    #[serde(default)]
    pub required: Option<bool>,
    /// Override the inferred `data_type`. Validated against the known set
    /// when the file is loaded.
    #[serde(default, rename = "type")]
    pub data_type: Option<String>,
    /// Drop the column from the grid entirely.
    #[serde(default)]
    pub hidden: Option<bool>,
    /// Free-form description surfaced in tooltips / ER diagrams.
    #[serde(default)]
    pub comment: Option<String>,
}

const ALLOWED_TYPES: &[&str] = &[
    "string",
    "number",
    "boolean",
    "timestamp",
    "binary",
    "geopoint",
    "reference",
    "array",
    "map",
    "null",
    "mixed",
];

/// Resolve which schema-overrides file applies for the given (project,
/// database). Looks up `{project}_{database}.json` first, then `{project}.json`
/// as a fallback. Returns the first existing path or None.
///
/// `(default)` parens in the database id are stripped from the filename so
/// the most common case produces a clean `{project}_default.json`.
fn resolve_path(dir: &Path, project: &str, database: &str) -> Option<PathBuf> {
    let db_safe = database.trim_matches(|c| c == '(' || c == ')');
    let primary = dir.join(format!("{project}_{db_safe}.json"));
    if primary.is_file() {
        return Some(primary);
    }
    let fallback = dir.join(format!("{project}.json"));
    if fallback.is_file() {
        return Some(fallback);
    }
    None
}

/// Load and validate the schema-overrides file for a (project, database).
/// Returns Ok(None) if the dir setting is empty or no matching file exists
/// (silent — power-user-opt-in feature). Returns Err only on parse /
/// validation failures so the user sees a clear init-time error instead of
/// mysterious downstream behaviour.
pub fn load(
    dir: Option<&str>,
    project: &str,
    database: &str,
) -> Result<Option<SchemaOverrides>, String> {
    let Some(dir) = dir.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let dir = Path::new(dir);
    if !dir.is_dir() {
        return Err(format!(
            "schema_overrides_dir: '{}' is not a directory",
            dir.display()
        ));
    }
    let Some(path) = resolve_path(dir, project, database) else {
        return Ok(None);
    };
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("schema_overrides: read failed for {}: {e}", path.display()))?;
    let parsed: SchemaOverrides = serde_json::from_str(&raw)
        .map_err(|e| format!("schema_overrides: invalid JSON in {}: {e}", path.display()))?;
    validate(&parsed)?;
    Ok(Some(parsed))
}

fn validate(ov: &SchemaOverrides) -> Result<(), String> {
    for (collection, c) in &ov.collections {
        for (field, f) in c.fields.iter().chain(c.extra_fields.iter()) {
            if let Some(t) = &f.data_type {
                if !ALLOWED_TYPES.contains(&t.as_str()) {
                    return Err(format!(
                        "schema_overrides: collection '{collection}' field '{field}' has \
                         unknown type '{t}'. Allowed: {}",
                        ALLOWED_TYPES.join(", ")
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Apply overrides to a freshly-inferred column list. Mutates in place.
///
/// The order of operations matters:
/// 1. Drop hidden fields first (saves work on subsequent steps).
/// 2. Apply field-level overrides (required / type / comment).
/// 3. Append `extra_fields` that aren't already in the column list.
pub fn apply(columns: &mut Vec<ColumnInfo>, overrides: &SchemaOverrides, collection: &str) {
    let Some(c) = overrides.collections.get(collection) else {
        return;
    };

    // 1) Drop hidden fields.
    columns.retain(|col| {
        !c.fields
            .get(&col.name)
            .map(|f| f.hidden.unwrap_or(false))
            .unwrap_or(false)
    });

    // 2) Apply per-field overrides.
    for col in columns.iter_mut() {
        if let Some(f) = c.fields.get(&col.name) {
            if let Some(req) = f.required {
                col.is_nullable = !req;
            }
            if let Some(t) = &f.data_type {
                col.data_type = t.clone();
            }
            if let Some(comment) = &f.comment {
                col.comment = Some(comment.clone());
            }
        }
    }

    // 3) Append extra_fields not already present.
    for (name, f) in &c.extra_fields {
        if columns.iter().any(|c| c.name == *name) {
            continue;
        }
        if f.hidden.unwrap_or(false) {
            continue; // declaring it hidden makes no sense, but handle gracefully
        }
        columns.push(ColumnInfo {
            name: name.clone(),
            data_type: f.data_type.clone().unwrap_or_else(|| "string".to_string()),
            is_nullable: !f.required.unwrap_or(false),
            references: None,
            comment: f.comment.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: "string".into(),
            is_nullable: true,
            references: None,
            comment: None,
        }
    }

    fn overrides_with_field(coll: &str, field: &str, fo: FieldOverride) -> SchemaOverrides {
        let mut fields = HashMap::new();
        fields.insert(field.into(), fo);
        let mut collections = HashMap::new();
        collections.insert(
            coll.into(),
            CollectionOverride {
                fields,
                extra_fields: HashMap::new(),
            },
        );
        SchemaOverrides { collections }
    }

    #[test]
    fn required_true_makes_field_not_nullable() {
        let mut cols = vec![col("email")];
        let ov = overrides_with_field(
            "advisors",
            "email",
            FieldOverride {
                required: Some(true),
                ..Default::default()
            },
        );
        apply(&mut cols, &ov, "advisors");
        assert!(!cols[0].is_nullable);
    }

    #[test]
    fn required_false_keeps_field_nullable() {
        let mut cols = vec![ColumnInfo {
            is_nullable: false,
            ..col("email")
        }];
        let ov = overrides_with_field(
            "advisors",
            "email",
            FieldOverride {
                required: Some(false),
                ..Default::default()
            },
        );
        apply(&mut cols, &ov, "advisors");
        assert!(cols[0].is_nullable);
    }

    #[test]
    fn type_override_changes_data_type() {
        let mut cols = vec![ColumnInfo {
            data_type: "mixed".into(),
            ..col("score")
        }];
        let ov = overrides_with_field(
            "advisors",
            "score",
            FieldOverride {
                data_type: Some("number".into()),
                ..Default::default()
            },
        );
        apply(&mut cols, &ov, "advisors");
        assert_eq!(cols[0].data_type, "number");
    }

    #[test]
    fn hidden_drops_column() {
        let mut cols = vec![col("email"), col("internalNotes")];
        let ov = overrides_with_field(
            "advisors",
            "internalNotes",
            FieldOverride {
                hidden: Some(true),
                ..Default::default()
            },
        );
        apply(&mut cols, &ov, "advisors");
        assert_eq!(cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["email"]);
    }

    #[test]
    fn comment_attaches_to_column() {
        let mut cols = vec![col("email")];
        let ov = overrides_with_field(
            "advisors",
            "email",
            FieldOverride {
                comment: Some("Login identifier".into()),
                ..Default::default()
            },
        );
        apply(&mut cols, &ov, "advisors");
        assert_eq!(cols[0].comment.as_deref(), Some("Login identifier"));
    }

    #[test]
    fn extra_fields_get_appended() {
        let mut cols = vec![col("email")];
        let mut extra = HashMap::new();
        extra.insert(
            "rarelySet".into(),
            FieldOverride {
                data_type: Some("number".into()),
                required: Some(false),
                ..Default::default()
            },
        );
        let mut collections = HashMap::new();
        collections.insert(
            "advisors".into(),
            CollectionOverride {
                fields: HashMap::new(),
                extra_fields: extra,
            },
        );
        let ov = SchemaOverrides { collections };
        apply(&mut cols, &ov, "advisors");
        let added = cols.iter().find(|c| c.name == "rarelySet").unwrap();
        assert_eq!(added.data_type, "number");
        assert!(added.is_nullable);
    }

    #[test]
    fn extra_fields_skip_if_already_present() {
        let mut cols = vec![col("email")];
        let mut extra = HashMap::new();
        extra.insert("email".into(), FieldOverride::default());
        let mut collections = HashMap::new();
        collections.insert(
            "advisors".into(),
            CollectionOverride {
                fields: HashMap::new(),
                extra_fields: extra,
            },
        );
        let ov = SchemaOverrides { collections };
        apply(&mut cols, &ov, "advisors");
        assert_eq!(cols.len(), 1);
    }

    #[test]
    fn unknown_collection_is_a_no_op() {
        let mut cols = vec![col("email")];
        let ov = overrides_with_field(
            "other",
            "email",
            FieldOverride {
                required: Some(true),
                ..Default::default()
            },
        );
        apply(&mut cols, &ov, "advisors");
        assert!(cols[0].is_nullable);
    }

    #[test]
    fn validate_rejects_unknown_type() {
        let ov = overrides_with_field(
            "advisors",
            "email",
            FieldOverride {
                data_type: Some("fnord".into()),
                ..Default::default()
            },
        );
        let err = validate(&ov).unwrap_err();
        assert!(err.contains("fnord"));
        assert!(err.contains("Allowed:"));
    }

    #[test]
    fn validate_accepts_all_known_types() {
        for t in ALLOWED_TYPES {
            let ov = overrides_with_field(
                "advisors",
                "f",
                FieldOverride {
                    data_type: Some((*t).into()),
                    ..Default::default()
                },
            );
            assert!(validate(&ov).is_ok(), "type {t} should validate");
        }
    }

    #[test]
    fn load_returns_none_for_empty_dir() {
        assert!(load(None, "p", "default").unwrap().is_none());
        assert!(load(Some(""), "p", "default").unwrap().is_none());
    }

    #[test]
    fn load_errors_when_dir_does_not_exist() {
        let err = load(
            Some("/tmp/firestore-plugin-no-such-dir-xyz"),
            "p",
            "default",
        )
        .unwrap_err();
        assert!(err.contains("is not a directory"));
    }

    #[test]
    fn load_returns_none_when_no_matching_file() {
        let dir = tempdir("no-matching");
        let res = load(Some(dir.to_str().unwrap()), "myproj", "default")
            .unwrap();
        assert!(res.is_none());
        cleanup(&dir);
    }

    #[test]
    fn load_picks_project_database_specific_file() {
        let dir = tempdir("specific");
        std::fs::write(
            dir.join("myproj_default.json"),
            r#"{"collections":{"a":{"fields":{"x":{"required":true}}}}}"#,
        )
        .unwrap();
        let ov = load(Some(dir.to_str().unwrap()), "myproj", "(default)")
            .unwrap()
            .unwrap();
        assert!(ov.collections.contains_key("a"));
        cleanup(&dir);
    }

    #[test]
    fn load_falls_back_to_project_only_file() {
        let dir = tempdir("fallback");
        std::fs::write(
            dir.join("myproj.json"),
            r#"{"collections":{"b":{"fields":{}}}}"#,
        )
        .unwrap();
        let ov = load(Some(dir.to_str().unwrap()), "myproj", "(default)")
            .unwrap()
            .unwrap();
        assert!(ov.collections.contains_key("b"));
        cleanup(&dir);
    }

    #[test]
    fn load_prefers_specific_over_fallback() {
        let dir = tempdir("prefer");
        std::fs::write(
            dir.join("myproj_default.json"),
            r#"{"collections":{"specific":{"fields":{}}}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("myproj.json"),
            r#"{"collections":{"generic":{"fields":{}}}}"#,
        )
        .unwrap();
        let ov = load(Some(dir.to_str().unwrap()), "myproj", "(default)")
            .unwrap()
            .unwrap();
        assert!(ov.collections.contains_key("specific"));
        assert!(!ov.collections.contains_key("generic"));
        cleanup(&dir);
    }

    #[test]
    fn load_surfaces_invalid_json() {
        let dir = tempdir("invalid");
        std::fs::write(dir.join("p.json"), "not json").unwrap();
        let err = load(Some(dir.to_str().unwrap()), "p", "default").unwrap_err();
        assert!(err.contains("invalid JSON"));
        cleanup(&dir);
    }

    fn tempdir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("firestore-plugin-test-{label}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &std::path::PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }
}
