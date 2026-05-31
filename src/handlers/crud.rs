//! Row-level CRUD over the Firestore document store.
//!
//! All three handlers share a common shape:
//! 1. Resolve table + doc-id + schema from params (with structured -32602 errors).
//! 2. Coerce edit-cell JSON → Firestore proto values via `crate::coercion`,
//!    using the inferred schema's `data_type` as a hint where available.
//! 3. Issue the proto-level RPC (create_doc / update_doc / delete_by_id)
//!    against the schema's target database.
//! 4. Invalidate COUNT_CACHE + CURSOR_CACHE for the (database, table) tuple.

use std::collections::HashMap;

use gcloud_sdk::google::firestore::v1::{Document, Value as ProtoValue};
use serde_json::{json, Value};

use crate::rpc::{error_response, ok_response};
use crate::state::SchemaCacheKey;

fn extract_schema(params: &Value) -> Option<String> {
    params
        .get("schema")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

pub async fn insert_record(id: Value, params: &Value) -> Value {
    let Some(table) = params.get("table").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing 'table' parameter", None);
    };
    let table = table.to_string();
    let Some(data) = params.get("data").and_then(Value::as_object) else {
        return error_response(id, -32602, "missing 'data' object", None);
    };

    let schema = extract_schema(params);
    let schema_name = crate::state::schema_or_default(schema.as_deref());

    if let Some(missing) = find_missing_required_fields(&schema_name, &table, data) {
        return error_response(
            id,
            -32602,
            &format!(
                "Required field(s) not set: {}. The plugin's schema declares \
                 these as is_nullable=false (likely via your schema-overrides \
                 file). Fill them in or mark the field optional in the override.",
                missing.join(", ")
            ),
            None,
        );
    }

    let db = match crate::client::resolve_for(id.clone(), schema.as_deref()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    let explicit_id: Option<String> = data
        .get(crate::schema_infer::ID_COLUMN)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let body: HashMap<String, Value> = data
        .iter()
        .filter(|(k, _)| k.as_str() != crate::schema_infer::ID_COLUMN)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let proto_fields = build_proto_fields(&schema_name, &table, &body);

    let new_doc = Document {
        name: String::new(),
        fields: proto_fields,
        create_time: None,
        update_time: None,
    };

    use firestore::FirestoreCreateSupport;
    let result = db
        .create_doc::<&str>(&table, explicit_id.as_deref(), new_doc, None)
        .await;

    let created = match result {
        Ok(d) => d,
        Err(e) => {
            let (code, msg, data) = crate::firestore_error::map_error(&e);
            return error_response(id, code, &msg, data);
        }
    };

    crate::state::invalidate_table_caches(&schema_name, &table);

    let _ = created;
    ok_response(id, json!(1u64))
}

pub async fn update_record(id: Value, params: &Value) -> Value {
    let Some(table) = params.get("table").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing 'table' parameter", None);
    };
    let table = table.to_string();
    let Some(pk_val) = params.get("pk_val").and_then(value_to_string) else {
        return error_response(id, -32602, "missing 'pk_val' parameter", None);
    };
    let Some(col_name) = params.get("col_name").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing 'col_name' parameter", None);
    };
    let col_name = col_name.to_string();
    let new_val = params.get("new_val").cloned().unwrap_or(Value::Null);

    let schema = extract_schema(params);
    let schema_name = crate::state::schema_or_default(schema.as_deref());

    let db = match crate::client::resolve_for(id.clone(), schema.as_deref()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    let settings = match crate::state::settings() {
        Some(s) => s,
        None => return error_response(id, -32602, "plugin not initialised", None),
    };

    if col_name == crate::schema_infer::ID_COLUMN {
        return rename_document(id, &db, &schema_name, &table, &pk_val, &new_val).await;
    }

    let mut single_field = HashMap::new();
    let hint = column_hint(&schema_name, &table, &col_name);
    single_field.insert(
        col_name.clone(),
        crate::coercion::json_to_proto(&new_val, hint.as_deref()),
    );

    // Document path references the schema's target database, not always the
    // connection's configured default.
    let doc_path = format!(
        "projects/{}/databases/{}/documents/{}/{}",
        settings.project_id, schema_name, table, pk_val
    );
    let doc = Document {
        name: doc_path,
        fields: single_field,
        create_time: None,
        update_time: None,
    };

    use firestore::FirestoreUpdateSupport;
    let result = db
        .update_doc(&table, doc, Some(vec![col_name]), None, None)
        .await;

    if let Err(e) = result {
        let (code, msg, data) = crate::firestore_error::map_error(&e);
        return error_response(id, code, &msg, data);
    }

    crate::state::invalidate_table_caches(&schema_name, &table);
    ok_response(id, json!(1u64))
}

pub async fn delete_record(id: Value, params: &Value) -> Value {
    let Some(table) = params.get("table").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing 'table' parameter", None);
    };
    let table = table.to_string();
    let Some(pk_val) = params.get("pk_val").and_then(value_to_string) else {
        return error_response(id, -32602, "missing 'pk_val' parameter", None);
    };

    let schema = extract_schema(params);
    let schema_name = crate::state::schema_or_default(schema.as_deref());

    let db = match crate::client::resolve_for(id.clone(), schema.as_deref()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    use firestore::FirestoreDeleteSupport;
    let result = db.delete_by_id(&table, &pk_val, None).await;

    if let Err(e) = result {
        let (code, msg, data) = crate::firestore_error::map_error(&e);
        return error_response(id, code, &msg, data);
    }

    crate::state::invalidate_table_caches(&schema_name, &table);
    ok_response(id, json!(1u64))
}

fn build_proto_fields(
    database_id: &str,
    table: &str,
    body: &HashMap<String, Value>,
) -> HashMap<String, ProtoValue> {
    let key = SchemaCacheKey {
        database_id: database_id.to_string(),
        table: table.to_string(),
    };
    let cache = crate::state::schema_cache_read();
    let columns = cache.get(&key).cloned();
    drop(cache);
    coerce_body(body, columns.as_deref())
}

fn coerce_body(
    body: &HashMap<String, Value>,
    columns: Option<&[crate::schema_infer::ColumnInfo]>,
) -> HashMap<String, ProtoValue> {
    body.iter()
        .map(|(name, value)| {
            let hint = columns
                .and_then(|cols| cols.iter().find(|c| &c.name == name))
                .map(|c| c.data_type.as_str());
            (name.clone(), crate::coercion::json_to_proto(value, hint))
        })
        .collect()
}

fn column_hint(database_id: &str, table: &str, col_name: &str) -> Option<String> {
    let key = SchemaCacheKey {
        database_id: database_id.to_string(),
        table: table.to_string(),
    };
    crate::state::schema_cache_read()
        .get(&key)?
        .iter()
        .find(|c| c.name == col_name)
        .map(|c| c.data_type.clone())
}

/// Firestore doesn't support in-place document-id renames, but the user
/// expectation is "I edited the id cell, save it." Implement that as a
/// best-effort read→create-at-new-id→delete-old sequence.
///
/// Caveats:
///   - Non-atomic: if the create succeeds and the delete fails, the user
///     ends up with a duplicate doc at both ids.
///   - Subcollections under the source doc are NOT moved.
///   - Reference fields in OTHER docs pointing at the old id keep pointing
///     at it.
async fn rename_document(
    id: Value,
    db: &firestore::FirestoreDb,
    database_id: &str,
    table: &str,
    old_id: &str,
    new_val: &Value,
) -> Value {
    let new_id = match new_val.as_str().filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => {
            return error_response(
                id,
                -32602,
                "renaming an id requires a non-empty string. To clear the id, \
                 delete the document instead.",
                None,
            )
        }
    };

    if new_id == old_id {
        return ok_response(id, json!(1u64));
    }

    use firestore::{FirestoreCreateSupport, FirestoreDeleteSupport, FirestoreGetByIdSupport};

    let source = match db.get_doc(table, old_id, None).await {
        Ok(d) => d,
        Err(e) => {
            let (code, msg, data) = crate::firestore_error::map_error(&e);
            return error_response(id, code, &msg, data);
        }
    };

    if db.get_doc(table, new_id, None).await.is_ok() {
        return error_response(
            id,
            -32602,
            &format!(
                "Cannot rename to '{new_id}': a document with that id already \
                 exists. Pick a different id or delete the existing one first."
            ),
            None,
        );
    }

    let new_doc = firestore::FirestoreDocument {
        name: String::new(),
        fields: source.fields,
        create_time: None,
        update_time: None,
    };
    if let Err(e) = db
        .create_doc::<&str>(table, Some(new_id), new_doc, None)
        .await
    {
        let (code, msg, data) = crate::firestore_error::map_error(&e);
        return error_response(id, code, &msg, data);
    }

    if let Err(e) = db.delete_by_id(table, old_id, None).await {
        let (code, msg, data) = crate::firestore_error::map_error(&e);
        return error_response(
            id,
            code,
            &format!(
                "Renamed copy created at '{new_id}' but failed to delete the \
                 source at '{old_id}': {msg}. You now have both — delete one \
                 manually."
            ),
            data,
        );
    }

    crate::state::invalidate_table_caches(database_id, table);
    ok_response(id, json!(1u64))
}

fn find_missing_required_fields(
    database_id: &str,
    table: &str,
    data: &serde_json::Map<String, Value>,
) -> Option<Vec<String>> {
    let key = SchemaCacheKey {
        database_id: database_id.to_string(),
        table: table.to_string(),
    };
    let cache = crate::state::schema_cache_read();
    let columns = cache.get(&key)?.clone();
    drop(cache);
    let missing = required_fields_missing(&columns, data);
    if missing.is_empty() {
        None
    } else {
        Some(missing)
    }
}

fn required_fields_missing(
    columns: &[crate::schema_infer::ColumnInfo],
    data: &serde_json::Map<String, Value>,
) -> Vec<String> {
    columns
        .iter()
        .filter(|c| !c.is_nullable && c.name != crate::schema_infer::ID_COLUMN)
        .filter(|c| match data.get(&c.name) {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            _ => false,
        })
        .map(|c| c.name.clone())
        .collect()
}

fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_infer::ColumnInfo;
    use gcloud_sdk::google::firestore::v1::value::ValueType;
    use serde_json::json;

    fn col(name: &str, data_type: &str, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: data_type.into(),
            is_nullable: nullable,
            references: None,
            comment: None,
        }
    }

    #[test]
    fn value_to_string_strings() {
        assert_eq!(value_to_string(&json!("abc")), Some("abc".to_string()));
    }

    #[test]
    fn value_to_string_rejects_empty_string() {
        assert_eq!(value_to_string(&json!("")), None);
    }

    #[test]
    fn value_to_string_accepts_numbers() {
        assert_eq!(value_to_string(&json!(42)), Some("42".to_string()));
        assert_eq!(value_to_string(&json!(2.5)), Some("2.5".to_string()));
    }

    #[test]
    fn value_to_string_rejects_null_bool_array() {
        assert_eq!(value_to_string(&Value::Null), None);
        assert_eq!(value_to_string(&json!(true)), None);
        assert_eq!(value_to_string(&json!([])), None);
    }

    #[test]
    fn required_fields_missing_returns_empty_when_all_set() {
        let cols = vec![
            col("id", "string", false),
            col("name", "string", false),
            col("email", "string", true),
        ];
        let data = json!({"name": "Alice", "email": ""}).as_object().unwrap().clone();
        assert!(required_fields_missing(&cols, &data).is_empty());
    }

    #[test]
    fn required_fields_missing_skips_synthetic_id() {
        let cols = vec![col("id", "string", false)];
        let data = serde_json::Map::new();
        assert!(required_fields_missing(&cols, &data).is_empty());
    }

    #[test]
    fn required_fields_missing_flags_absent_field() {
        let cols = vec![col("name", "string", false)];
        let data = serde_json::Map::new();
        assert_eq!(required_fields_missing(&cols, &data), vec!["name"]);
    }

    #[test]
    fn required_fields_missing_flags_null_value() {
        let cols = vec![col("name", "string", false)];
        let data = json!({"name": null}).as_object().unwrap().clone();
        assert_eq!(required_fields_missing(&cols, &data), vec!["name"]);
    }

    #[test]
    fn required_fields_missing_flags_empty_string() {
        let cols = vec![col("name", "string", false)];
        let data = json!({"name": ""}).as_object().unwrap().clone();
        assert_eq!(required_fields_missing(&cols, &data), vec!["name"]);
    }

    #[test]
    fn required_fields_missing_treats_zero_and_false_as_set() {
        let cols = vec![
            col("count", "number", false),
            col("active", "boolean", false),
        ];
        let data = json!({"count": 0, "active": false}).as_object().unwrap().clone();
        assert!(required_fields_missing(&cols, &data).is_empty());
    }

    #[test]
    fn required_fields_missing_treats_empty_collections_as_set() {
        let cols = vec![
            col("tags", "array", false),
            col("meta", "map", false),
        ];
        let data = json!({"tags": [], "meta": {}}).as_object().unwrap().clone();
        assert!(required_fields_missing(&cols, &data).is_empty());
    }

    #[test]
    fn required_fields_missing_returns_multiple() {
        let cols = vec![
            col("a", "string", false),
            col("b", "string", false),
            col("c", "string", false),
        ];
        let data = json!({"b": "x"}).as_object().unwrap().clone();
        let mut missing = required_fields_missing(&cols, &data);
        missing.sort();
        assert_eq!(missing, vec!["a", "c"]);
    }

    #[test]
    fn coerce_body_uses_column_hint_when_present() {
        let mut body = HashMap::new();
        body.insert("when".to_string(), json!("2026-05-09T10:00:00Z"));
        let cols = vec![col("when", "timestamp", true)];
        let proto = coerce_body(&body, Some(&cols));
        let when = proto.get("when").unwrap();
        assert!(matches!(when.value_type, Some(ValueType::TimestampValue(_))));
    }

    #[test]
    fn coerce_body_falls_back_to_string_without_hint() {
        let mut body = HashMap::new();
        body.insert("when".to_string(), json!("2026-05-09T10:00:00Z"));
        let proto = coerce_body(&body, None);
        let when = proto.get("when").unwrap();
        assert!(matches!(when.value_type, Some(ValueType::StringValue(_))));
    }

    #[test]
    fn coerce_body_handles_unknown_columns() {
        let mut body = HashMap::new();
        body.insert("ad_hoc".to_string(), json!(42));
        let cols = vec![col("known", "string", true)];
        let proto = coerce_body(&body, Some(&cols));
        let v = proto.get("ad_hoc").unwrap();
        assert!(matches!(v.value_type, Some(ValueType::IntegerValue(42))));
    }
}
