//! Schema metadata.
//!
//! Schema in Firestore terms = database. Each handler accepts an optional
//! `schema` param; when present it points to a different database_id under
//! the same project and is routed via `client::resolve_for(schema)`. When
//! absent (legacy host) the configured `settings.database_id` is used.

use futures::TryStreamExt;
use serde_json::{json, Value};

use crate::rpc::ok_response;
use crate::state::SchemaCacheKey;

fn extract_schema(params: &Value) -> Option<String> {
    params
        .get("schema")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

pub async fn get_databases(id: Value, params: &Value) -> Value {
    list_databases(id, params).await
}

/// Connection-form discovery RPC: enumerate every Firestore database the
/// caller's credential can see under `settings.project_id`.
pub async fn list_databases(id: Value, _params: &Value) -> Value {
    let Some(settings) = crate::state::settings() else {
        return crate::rpc::error_response(id, -32602, "plugin not initialised", None);
    };
    match crate::admin::list_databases(settings).await {
        Ok(ids) => ok_response(id, json!(ids)),
        Err(err) => crate::rpc::error_response(id, err.code, &err.message, None),
    }
}

pub async fn get_schemas(id: Value, _params: &Value) -> Value {
    let Some(settings) = crate::state::settings() else {
        return crate::rpc::error_response(id, -32602, "plugin not initialised", None);
    };
    match crate::admin::list_databases(settings).await {
        Ok(ids) => {
            let schemas: Vec<Value> = ids
                .into_iter()
                .map(|name| json!({ "name": name, "comment": Value::Null }))
                .collect();
            ok_response(id, Value::Array(schemas))
        }
        Err(err) => crate::rpc::error_response(id, err.code, &err.message, None),
    }
}

pub async fn get_tables(id: Value, params: &Value) -> Value {
    let schema = extract_schema(params);
    let db = match crate::client::resolve_for(id.clone(), schema.as_deref()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    let schema_name = crate::state::schema_or_default(schema.as_deref());

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
        .map(|n| json!({ "name": n, "schema": schema_name, "comment": Value::Null }))
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
    let schema = extract_schema(params);
    let schema_name = crate::state::schema_or_default(schema.as_deref());
    let key = SchemaCacheKey {
        database_id: schema_name.clone(),
        table: table.clone(),
    };

    if let Some(cached) = crate::state::schema_cache_read().get(&key) {
        let cols: Vec<Value> = cached.iter().map(|c| c.to_json()).collect();
        return ok_response(id, json!(cols));
    }

    let db = match crate::client::resolve_for(id.clone(), schema.as_deref()).await {
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
    crate::state::schema_cache_write().insert(key, columns.clone());

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

pub async fn get_schema_snapshot(id: Value, params: &Value) -> Value {
    let schema = extract_schema(params);
    let schema_name = crate::state::schema_or_default(schema.as_deref());
    let db = match crate::client::resolve_for(id.clone(), schema.as_deref()).await {
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

    {
        let mut cache = crate::state::schema_cache_write();
        for (table, columns) in &fetched {
            cache.insert(
                SchemaCacheKey {
                    database_id: schema_name.clone(),
                    table: table.clone(),
                },
                columns.clone(),
            );
        }
    }

    // Tabularis' plugin-driver bridge expects `Vec<TableSchema>`:
    //   [{ name, columns: TableColumn[], foreign_keys: ForeignKey[] }, ...]
    // (verified in src-tauri/src/plugins/driver.rs:606 and types/editor.ts).
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

    let schema = extract_schema(params);
    let schema_name = crate::state::schema_or_default(schema.as_deref());

    let mut result: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut to_fetch: Vec<String> = Vec::new();
    {
        let cache = crate::state::schema_cache_read();
        for table in &tables {
            let key = SchemaCacheKey {
                database_id: schema_name.clone(),
                table: table.clone(),
            };
            if let Some(cols) = cache.get(&key) {
                let json_cols: Vec<Value> = cols.iter().map(|c| c.to_json()).collect();
                result.insert(table.clone(), Value::Array(json_cols));
            } else {
                to_fetch.push(table.clone());
            }
        }
    }

    if !to_fetch.is_empty() {
        let db = match crate::client::resolve_for(id.clone(), schema.as_deref()).await {
            Ok(db) => db,
            Err(resp) => return resp,
        };
        let n = crate::state::settings()
            .map(|s| s.sample_size)
            .unwrap_or(50);

        use futures::stream::StreamExt;
        let fetches = futures::stream::iter(to_fetch.into_iter().map(|table| {
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
            }
        }))
        .buffer_unordered(8);

        let fetched: Vec<_> = fetches.collect().await;
        let mut cache = crate::state::schema_cache_write();
        for (table, columns) in fetched {
            let json_cols: Vec<Value> = columns.iter().map(|c| c.to_json()).collect();
            result.insert(table.clone(), Value::Array(json_cols));
            cache.insert(
                SchemaCacheKey {
                    database_id: schema_name.clone(),
                    table,
                },
                columns,
            );
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
