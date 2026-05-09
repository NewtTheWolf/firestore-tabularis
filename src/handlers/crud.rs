//! Row-level CRUD over the Firestore document store.
//!
//! All three handlers share a common shape:
//! 1. Resolve table + doc-id from params (with structured -32602 errors).
//! 2. Coerce edit-cell JSON → Firestore proto values via `crate::coercion`,
//!    using the inferred schema's `data_type` as a hint where available.
//! 3. Issue the proto-level RPC (create_doc / update_doc / delete_by_id).
//! 4. Invalidate COUNT_CACHE + CURSOR_CACHE for the touched table.

use std::collections::HashMap;

use gcloud_sdk::google::firestore::v1::{Document, Value as ProtoValue};
use serde_json::{json, Value};

use crate::rpc::{error_response, ok_response};

pub async fn insert_record(id: Value, params: &Value) -> Value {
    let Some(table) = params.get("table").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing 'table' parameter", None);
    };
    let table = table.to_string();
    let Some(data) = params.get("data").and_then(Value::as_object) else {
        return error_response(id, -32602, "missing 'data' object", None);
    };

    let db = match crate::client::resolve(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    let settings = match crate::state::settings() {
        Some(s) => s,
        None => return error_response(id, -32602, "plugin not initialised", None),
    };

    // Pull doc-id out of the payload if the user supplied it; the rest of the
    // map becomes the document body. An empty/missing id triggers Firestore's
    // server-side ID generation.
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

    let proto_fields = build_proto_fields(&table, &body);

    // The doc name for a Create with a known ID; for autogen we leave it empty
    // and let the server fill it in. firestore-rs takes Option<S> for the id.
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

    crate::state::invalidate_table_caches(&table);

    let new_id = created.name.rsplit('/').next().unwrap_or("").to_string();
    let _ = settings; // suppress unused warning when settings is only validated above
    ok_response(
        id,
        json!({
            "affected_rows": 1,
            "id": new_id,
        }),
    )
}

pub async fn update_record(id: Value, params: &Value) -> Value {
    let Some(table) = params.get("table").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing 'table' parameter", None);
    };
    let table = table.to_string();
    let Some(pk_val) = params.get("pkVal").and_then(value_to_string) else {
        return error_response(id, -32602, "missing 'pkVal' parameter", None);
    };
    let Some(col_name) = params.get("colName").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing 'colName' parameter", None);
    };
    let col_name = col_name.to_string();
    let new_val = params.get("newVal").cloned().unwrap_or(Value::Null);

    if col_name == crate::schema_infer::ID_COLUMN {
        return error_response(
            id,
            -32602,
            "Cannot rename document ID via update — Firestore doesn't support \
             in-place doc-id changes. Delete + re-insert with the new id instead.",
            None,
        );
    }

    let db = match crate::client::resolve(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    let settings = match crate::state::settings() {
        Some(s) => s,
        None => return error_response(id, -32602, "plugin not initialised", None),
    };

    // Build a single-field document and tell Firestore to update only that field.
    let mut single_field = HashMap::new();
    let hint = column_hint(&table, &col_name);
    single_field.insert(
        col_name.clone(),
        crate::coercion::json_to_proto(&new_val, hint.as_deref()),
    );

    let doc_path = format!(
        "projects/{}/databases/{}/documents/{}/{}",
        settings.project_id, settings.database_id, table, pk_val
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

    crate::state::invalidate_table_caches(&table);
    ok_response(id, json!({ "affected_rows": 1 }))
}

pub async fn delete_record(id: Value, params: &Value) -> Value {
    let Some(table) = params.get("table").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing 'table' parameter", None);
    };
    let table = table.to_string();
    let Some(pk_val) = params.get("pkVal").and_then(value_to_string) else {
        return error_response(id, -32602, "missing 'pkVal' parameter", None);
    };

    let db = match crate::client::resolve(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    use firestore::FirestoreDeleteSupport;
    let result = db.delete_by_id(&table, &pk_val, None).await;

    if let Err(e) = result {
        let (code, msg, data) = crate::firestore_error::map_error(&e);
        return error_response(id, code, &msg, data);
    }

    crate::state::invalidate_table_caches(&table);
    ok_response(id, json!({ "affected_rows": 1 }))
}

/// Build a Firestore proto-fields map from the JSON edit-cell payload, using
/// the cached schema for type hints.
fn build_proto_fields(
    table: &str,
    body: &HashMap<String, Value>,
) -> HashMap<String, ProtoValue> {
    let cache = crate::state::schema_cache_read();
    let columns = cache.get(table).cloned();
    drop(cache);
    body.iter()
        .map(|(name, value)| {
            let hint = columns
                .as_ref()
                .and_then(|cols| cols.iter().find(|c| &c.name == name))
                .map(|c| c.data_type.as_str());
            (name.clone(), crate::coercion::json_to_proto(value, hint))
        })
        .collect()
}

fn column_hint(table: &str, col_name: &str) -> Option<String> {
    crate::state::schema_cache_read()
        .get(table)?
        .iter()
        .find(|c| c.name == col_name)
        .map(|c| c.data_type.clone())
}

/// Coerce a JSON value into the string form Firestore uses for doc IDs.
/// Accepts strings (most common) and numbers (which Tabularis sometimes sends
/// when the synthetic `id` column carries a numeric-looking value).
fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}
