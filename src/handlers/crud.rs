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

    // Required-field validation. Tabularis' NewRowModal silently drops empty
    // required fields from the payload (relational drivers rely on the DB
    // server to fail the insert with NOT NULL — Firestore happily accepts the
    // partial doc). We catch missing/empty required fields here so the user
    // sees a clear error instead of a silent success with bad data.
    if let Some(missing) = find_missing_required_fields(&table, data) {
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

    let db = match crate::client::resolve(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
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

    // Tabularis' plugin driver expects a bare u64 (affected rows) — the new
    // doc-id from `created.name` would be useful but isn't part of the
    // contract; Tabularis re-fetches the table list.
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

    let db = match crate::client::resolve(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    let settings = match crate::state::settings() {
        Some(s) => s,
        None => return error_response(id, -32602, "plugin not initialised", None),
    };

    if col_name == crate::schema_infer::ID_COLUMN {
        return rename_document(id, db, &table, &pk_val, &new_val).await;
    }

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
    ok_response(id, json!(1u64))
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

/// Firestore doesn't support in-place document-id renames, but the user
/// expectation is "I edited the id cell, save it." Implement that as a
/// best-effort read→create-at-new-id→delete-old sequence.
///
/// Caveats (returned to the caller as warnings would be ideal but Tabularis'
/// update_record contract is `Result<u64, String>` — no warning channel):
///   - Non-atomic: if the create succeeds and the delete fails, the user
///     ends up with a duplicate doc at both ids. Failure is rare (network
///     blip mid-rename) and the user can clean up manually.
///   - Subcollections under the source doc are NOT moved — they stay
///     orphaned under the now-deleted parent path. Phase 4 will surface
///     this as a UI confirmation when subcollections are detected.
///   - Reference fields in OTHER docs pointing at the old id keep pointing
///     at it — they go stale. No global rewrite (would require a full scan).
async fn rename_document(
    id: Value,
    db: &firestore::FirestoreDb,
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
        // No-op — same id. Idempotent: report success without doing work.
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

    crate::state::invalidate_table_caches(table);
    ok_response(id, json!(1u64))
}

/// Validate that every column declared `is_nullable=false` (other than the
/// synthetic `id`, which Firestore generates if absent) is present and
/// non-empty in the insert payload. Returns the list of missing field names
/// when validation fails, or None when everything is fine.
///
/// "Empty" means: missing from the map, JSON null, or empty string. Boolean
/// false, numeric 0, empty array/object are all treated as set — those are
/// legitimate values for typed columns.
fn find_missing_required_fields(
    table: &str,
    data: &serde_json::Map<String, Value>,
) -> Option<Vec<String>> {
    let cache = crate::state::schema_cache_read();
    let columns = cache.get(table)?.clone();
    drop(cache);

    let missing: Vec<String> = columns
        .iter()
        .filter(|c| !c.is_nullable && c.name != crate::schema_infer::ID_COLUMN)
        .filter(|c| match data.get(&c.name) {
            None | Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            _ => false,
        })
        .map(|c| c.name.clone())
        .collect();

    if missing.is_empty() {
        None
    } else {
        Some(missing)
    }
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
