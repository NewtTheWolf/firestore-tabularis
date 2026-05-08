//! Schema metadata.

use futures::TryStreamExt;
use serde_json::{json, Value};

use crate::rpc::ok_response;

pub async fn get_databases(id: Value, _params: &Value) -> Value {
    let Some(settings) = crate::state::settings() else {
        return crate::rpc::error_response(id, -32602, "plugin not initialised", None);
    };
    ok_response(id, json!([settings.database_id]))
}

pub fn get_schemas(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}

pub async fn get_tables(id: Value, _params: &Value) -> Value {
    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    let stream = match db
        .fluent()
        .list()
        .collections()
        .stream_all_with_errors()
        .await
    {
        Ok(s) => s,
        Err(e) => return error_from(id, &e),
    };

    let names: Vec<String> = match stream.try_collect().await {
        Ok(v) => v,
        Err(e) => return error_from(id, &e),
    };

    let mut tables: Vec<Value> = names
        .into_iter()
        .map(|n| json!({ "name": n, "schema": Value::Null, "comment": Value::Null }))
        .collect();
    tables.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    ok_response(id, json!(tables))
}

async fn resolve_client(id: Value) -> Result<&'static firestore::FirestoreDb, Value> {
    let Some(settings) = crate::state::settings() else {
        return Err(crate::rpc::error_response(
            id,
            -32602,
            "plugin not initialised",
            None,
        ));
    };
    crate::state::CLIENT
        .get_or_try_init(|| async { crate::client::build(settings).await })
        .await
        .map_err(|err| crate::rpc::error_response(id.clone(), err.code, &err.message, None))
}

fn error_from(id: Value, err: &firestore::errors::FirestoreError) -> Value {
    let (code, msg, data) = crate::firestore_error::map_error(err);
    crate::rpc::error_response(id, code, &msg, data)
}

pub async fn get_columns(id: Value, params: &Value) -> Value {
    let table = params
        .get("table")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if table.is_empty() {
        return crate::rpc::error_response(id, -32602, "missing 'table' parameter", None);
    }

    if let Some(cached) = crate::state::SCHEMA_CACHE.read().unwrap().get(&table) {
        let cols: Vec<Value> = cached.iter().map(|c| c.to_json()).collect();
        return ok_response(id, json!(cols));
    }

    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    let n = crate::state::settings()
        .map(|s| s.sample_size)
        .unwrap_or(50);

    let docs: Vec<firestore::FirestoreDocument> = match db
        .fluent()
        .select()
        .from(table.as_str())
        .limit(n)
        .query()
        .await
    {
        Ok(d) => d,
        Err(e) => return error_from(id, &e),
    };

    let sample: Vec<crate::schema_infer::DocumentTypes> = docs
        .iter()
        .map(crate::schema_infer::types_from_document)
        .collect();

    let columns = crate::schema_infer::infer(&sample);
    crate::state::SCHEMA_CACHE
        .write()
        .unwrap()
        .insert(table, columns.clone());

    let json_cols: Vec<Value> = columns.iter().map(|c| c.to_json()).collect();
    ok_response(id, json!(json_cols))
}

pub fn get_foreign_keys(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}
pub fn get_indexes(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}
pub fn get_views(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}
pub fn get_view_definition(id: Value, _params: &Value) -> Value {
    ok_response(id, Value::String(String::new()))
}
pub fn get_view_columns(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}
pub fn get_routines(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}
pub fn get_routine_parameters(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}
pub fn get_routine_definition(id: Value, _params: &Value) -> Value {
    ok_response(id, Value::String(String::new()))
}

pub fn get_schema_snapshot(id: Value, _params: &Value) -> Value {
    ok_response(
        id,
        json!({ "tables": [], "columns": {}, "foreign_keys": {} }),
    )
}

pub fn get_all_columns_batch(id: Value, _params: &Value) -> Value {
    ok_response(id, json!({}))
}
pub fn get_all_foreign_keys_batch(id: Value, _params: &Value) -> Value {
    ok_response(id, json!({}))
}
