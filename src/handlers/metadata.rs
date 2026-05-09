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
    let db = match crate::client::resolve(id.clone()).await {
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

    if let Some(cached) = crate::state::schema_cache_read().get(&table) {
        let cols: Vec<Value> = cached.iter().map(|c| c.to_json()).collect();
        return ok_response(id, json!(cols));
    }

    let db = match crate::client::resolve(id.clone()).await {
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
    let refs: Vec<crate::schema_infer::DocumentReferences> = docs
        .iter()
        .map(crate::schema_infer::references_from_document)
        .collect();

    let mut columns = crate::schema_infer::infer(&sample, &refs);
    if let Some(ov) = crate::state::schema_overrides() {
        crate::schema_overrides::apply(&mut columns, ov, &table);
    }
    crate::state::schema_cache_write().insert(table, columns.clone());

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

pub async fn get_schema_snapshot(id: Value, _params: &Value) -> Value {
    let db = match crate::client::resolve(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    // List all root collections.
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

    let table_names: Vec<String> = match stream.try_collect().await {
        Ok(v) => v,
        Err(e) => return error_from(id, &e),
    };

    let n = crate::state::settings()
        .map(|s| s.sample_size)
        .unwrap_or(50);

    // Parallel fetch for every collection, throttled to 8 concurrent gRPC
    // calls. Unbounded fan-out on a project with hundreds of collections
    // would exhaust the shared channel and trip the Firestore quota limiter.
    use futures::stream::StreamExt;
    let fetches = futures::stream::iter(table_names.iter().cloned().map(|table| {
        let db = db.clone();
        async move {
            let docs: Vec<firestore::FirestoreDocument> = db
                .fluent()
                .select()
                .from(table.as_str())
                .limit(n)
                .query()
                .await
                .unwrap_or_default();
            let types: Vec<crate::schema_infer::DocumentTypes> = docs
                .iter()
                .map(crate::schema_infer::types_from_document)
                .collect();
            let refs: Vec<crate::schema_infer::DocumentReferences> = docs
                .iter()
                .map(crate::schema_infer::references_from_document)
                .collect();
            let mut columns = crate::schema_infer::infer(&types, &refs);
            if let Some(ov) = crate::state::schema_overrides() {
                crate::schema_overrides::apply(&mut columns, ov, &table);
            }
            (table, columns)
        }
    }))
    .buffer_unordered(8);
    let fetched: Vec<(String, Vec<crate::schema_infer::ColumnInfo>)> = fetches.collect().await;

    // Snapshot results are valuable for subsequent get_columns calls — fill
    // the cache so we don't re-infer the same schema right after.
    {
        let mut cache = crate::state::schema_cache_write();
        for (table, columns) in &fetched {
            cache.insert(table.clone(), columns.clone());
        }
    }

    // Tabularis' plugin-driver bridge expects `Vec<TableSchema>`:
    //   [{ name, columns: TableColumn[], foreign_keys: ForeignKey[] }, ...]
    // (verified in src-tauri/src/plugins/driver.rs:606 and types/editor.ts).
    // Each ForeignKey is { name, column_name, ref_table, ref_column }.
    let mut tables_out: Vec<Value> = fetched
        .into_iter()
        .map(|(table, columns)| {
            let cols_arr: Vec<Value> = columns.iter().map(|c| c.to_json()).collect();
            let fks: Vec<Value> = columns
                .iter()
                .filter_map(|c| {
                    c.references.as_ref().map(|target| {
                        json!({
                            "name": format!("fk_{}_{}", table, c.name),
                            "column_name": c.name.clone(),
                            "ref_table": target.clone(),
                            "ref_column": crate::schema_infer::ID_COLUMN,
                        })
                    })
                })
                .collect();
            json!({
                "name": table,
                "columns": cols_arr,
                "foreign_keys": fks,
            })
        })
        .collect();
    tables_out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

    ok_response(id, Value::Array(tables_out))
}

pub async fn get_all_columns_batch(id: Value, params: &Value) -> Value {
    let tables: Vec<String> = params
        .get("tables")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    if tables.is_empty() {
        return ok_response(id, json!({}));
    }

    let mut result: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut to_fetch: Vec<String> = Vec::new();
    {
        let cache = crate::state::schema_cache_read();
        for table in &tables {
            if let Some(cols) = cache.get(table) {
                let json_cols: Vec<Value> = cols.iter().map(|c| c.to_json()).collect();
                result.insert(table.clone(), Value::Array(json_cols));
            } else {
                to_fetch.push(table.clone());
            }
        }
    }

    if !to_fetch.is_empty() {
        let db = match crate::client::resolve(id.clone()).await {
            Ok(db) => db,
            Err(resp) => return resp,
        };
        let n = crate::state::settings()
            .map(|s| s.sample_size)
            .unwrap_or(50);

        use futures::stream::StreamExt;
        let fetches = futures::stream::iter(to_fetch.into_iter().map(|table| async move {
            let docs: Vec<firestore::FirestoreDocument> = db
                .fluent()
                .select()
                .from(table.as_str())
                .limit(n)
                .query()
                .await
                .unwrap_or_default();
            let sample: Vec<crate::schema_infer::DocumentTypes> = docs
                .iter()
                .map(crate::schema_infer::types_from_document)
                .collect();
            let refs: Vec<crate::schema_infer::DocumentReferences> = docs
                .iter()
                .map(crate::schema_infer::references_from_document)
                .collect();
            let mut columns = crate::schema_infer::infer(&sample, &refs);
            if let Some(ov) = crate::state::schema_overrides() {
                crate::schema_overrides::apply(&mut columns, ov, &table);
            }
            (table, columns)
        }))
        .buffer_unordered(8);

        let fetched: Vec<_> = fetches.collect().await;
        let mut cache = crate::state::schema_cache_write();
        for (table, columns) in fetched {
            let json_cols: Vec<Value> = columns.iter().map(|c| c.to_json()).collect();
            result.insert(table.clone(), Value::Array(json_cols));
            cache.insert(table, columns);
        }
    }

    ok_response(id, Value::Object(result))
}

pub fn get_all_foreign_keys_batch(id: Value, params: &Value) -> Value {
    let tables: Vec<String> = params
        .get("tables")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut result = serde_json::Map::new();
    for t in tables {
        result.insert(t, json!([]));
    }
    ok_response(id, Value::Object(result))
}
