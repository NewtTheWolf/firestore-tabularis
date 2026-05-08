//! Connection and query execution.

use serde_json::{json, Value};

use crate::rpc::ok_response;

pub async fn initialize(id: Value, params: &Value) -> Value {
    let settings_value = params.get("settings").cloned().unwrap_or(Value::Null);
    let settings = crate::models::Settings::from_value(&settings_value);
    let _ = crate::state::SETTINGS.set(settings); // second initialize is a no-op
    ok_response(id, Value::Null)
}

pub async fn ping(id: Value, params: &Value) -> Value {
    if crate::state::CLIENT.get().is_some() {
        return ok_response(id, Value::Null);
    }
    test_connection(id, params).await
}

pub async fn test_connection(id: Value, _params: &Value) -> Value {
    let Some(settings) = crate::state::settings() else {
        return crate::rpc::error_response(
            id,
            -32602,
            "plugin not initialised — host should send 'initialize' before 'test_connection'",
            None,
        );
    };

    // Preserve PluginError's code through tokio::sync::OnceCell. The closure must
    // return a single error type; we use PluginError directly (not String) so the
    // structured code (-32602 invalid_params vs -32603 internal) survives.
    let result = crate::state::CLIENT
        .get_or_try_init(|| async { crate::client::build(settings).await })
        .await;

    match result {
        Ok(db) => {
            // Cheap probe: list root collections. We don't care about the results, only
            // that the call succeeds — this verifies the project_id is reachable and the
            // credential has read permission, beyond what FirestoreDb::with_options checks.
            use futures::TryStreamExt;
            match db
                .fluent()
                .list()
                .collections()
                .stream_all_with_errors()
                .await
            {
                Ok(stream) => match stream.try_collect::<Vec<_>>().await {
                    Ok(_) => ok_response(id, json!({ "success": true })),
                    Err(e) => {
                        let (code, msg, data) = crate::firestore_error::map_error(&e);
                        crate::rpc::error_response(id, code, &msg, data)
                    }
                },
                Err(e) => {
                    let (code, msg, data) = crate::firestore_error::map_error(&e);
                    crate::rpc::error_response(id, code, &msg, data)
                }
            }
        }
        Err(err) => crate::rpc::error_response(id, err.code, &err.message, None),
    }
}

pub async fn execute_query(id: Value, params: &Value) -> Value {
    let sql = params
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let parsed = match crate::query_parser::parse(&sql) {
        Ok(p) => p,
        Err(e) => return crate::rpc::error_response(id, -32602, &e, None),
    };

    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    let order_items: Vec<(String, firestore::FirestoreQueryDirection)> = parsed
        .order_by
        .iter()
        .map(|i| {
            (
                i.field.clone(),
                if i.desc {
                    firestore::FirestoreQueryDirection::Descending
                } else {
                    firestore::FirestoreQueryDirection::Ascending
                },
            )
        })
        .collect();

    let mut q = db.fluent().select().from(parsed.table.as_str());
    if !order_items.is_empty() {
        q = q.order_by(order_items);
    }
    if let Some(n) = parsed.limit {
        q = q.limit(n as u32);
    }
    if let Some(o) = parsed.offset {
        q = q.offset(o as u32);
    }

    let started = std::time::Instant::now();
    let docs: Vec<firestore::FirestoreDocument> = match q.query().await {
        Ok(d) => d,
        Err(e) => return error_from_query(id, &e),
    };
    let elapsed = started.elapsed().as_millis() as u64;

    let columns = match crate::state::SCHEMA_CACHE
        .read()
        .unwrap()
        .get(&parsed.table)
    {
        Some(c) => c.clone(),
        None => {
            // Infer on the fly (caller will hit the cache next time via get_columns).
            let sample: Vec<_> = docs
                .iter()
                .map(crate::schema_infer::types_from_document)
                .collect();
            crate::schema_infer::infer(&sample)
        }
    };

    let column_names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let rows: Vec<Value> = docs.iter().map(|d| serialize_row(d, &columns)).collect();

    ok_response(
        id,
        json!({
            "columns": column_names,
            "rows": rows,
            "total_count": rows.len(),
            "affected_rows": 0,
            "execution_time_ms": elapsed,
        }),
    )
}

fn serialize_row(
    doc: &firestore::FirestoreDocument,
    columns: &[crate::schema_infer::ColumnInfo],
) -> Value {
    let id = doc_short_id(doc);
    let mut row: Vec<Value> = Vec::with_capacity(columns.len());
    for col in columns {
        if col.name == "__id__" {
            row.push(Value::String(id.clone()));
            continue;
        }
        match doc.fields.get(&col.name) {
            Some(v) => row.push(serialize_value(v)),
            None => row.push(Value::Null),
        }
    }
    Value::Array(row)
}

/// Last path segment of a doc's resource name — the human-friendly document ID.
fn doc_short_id(doc: &firestore::FirestoreDocument) -> String {
    doc.name.rsplit('/').next().unwrap_or("").to_string()
}

fn serialize_value(v: &gcloud_sdk::google::firestore::v1::Value) -> Value {
    use gcloud_sdk::google::firestore::v1::value::ValueType as V;
    match v.value_type.as_ref() {
        Some(V::NullValue(_)) | None => Value::Null,
        Some(V::BooleanValue(b)) => Value::Bool(*b),
        Some(V::IntegerValue(n)) => json!(n),
        Some(V::DoubleValue(f)) => json!(f),
        Some(V::StringValue(s)) => Value::String(s.clone()),
        Some(V::BytesValue(b)) => {
            use base64::Engine;
            Value::String(base64::engine::general_purpose::STANDARD.encode(b))
        }
        Some(V::TimestampValue(t)) => {
            // RFC 3339 via chrono. The `nanos` field on a Firestore Timestamp is i32 in [0, 1e9).
            let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(t.seconds, t.nanos as u32);
            match dt {
                Some(d) => Value::String(d.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
                None => Value::Null,
            }
        }
        Some(V::ReferenceValue(r)) => Value::String(r.clone()),
        Some(V::GeoPointValue(g)) => json!({ "lat": g.latitude, "lng": g.longitude }),
        Some(V::ArrayValue(a)) => {
            let items: Vec<Value> = a.values.iter().map(serialize_value).collect();
            Value::String(serde_json::to_string(&items).unwrap_or_default())
        }
        Some(V::MapValue(m)) => {
            let map: serde_json::Map<String, Value> = m
                .fields
                .iter()
                .map(|(k, x)| (k.clone(), serialize_value(x)))
                .collect();
            Value::String(serde_json::to_string(&Value::Object(map)).unwrap_or_default())
        }
        // Extra proto variants not used in standard Firestore storage:
        Some(V::FieldReferenceValue(_)) => Value::Null,
        Some(V::FunctionValue(_)) => Value::Null,
        Some(V::PipelineValue(_)) => Value::Null,
    }
}

fn error_from_query(id: Value, err: &firestore::errors::FirestoreError) -> Value {
    let (code, msg, data) = crate::firestore_error::map_error(err);
    crate::rpc::error_response(id, code, &msg, data)
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

pub fn explain_query(id: Value, _params: &Value) -> Value {
    crate::rpc::not_implemented(id, "explain_query")
}
