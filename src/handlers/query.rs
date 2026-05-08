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
    let mut parsed = match crate::query_parser::parse(&sql) {
        Ok(p) => p,
        Err(e) => return crate::rpc::error_response(id, -32602, &e, None),
    };

    if let (Some(filter), Some(settings)) = (parsed.where_clause.as_mut(), crate::state::settings())
    {
        crate::firestore_filter::rewrite_doc_id(
            filter,
            &parsed.table,
            &settings.project_id,
            &settings.database_id,
        );
    }

    // Pre-flight Firestore restriction validation (before any I/O).
    if let Some(filter) = &parsed.where_clause {
        if let Err(msg) = crate::firestore_filter::validate(filter) {
            return crate::rpc::error_response(id, -32602, &msg, None);
        }
    }

    // Resolve effective limit/offset: params.page/page_size override SQL.
    let host_page_size: Option<u64> = params.get("page_size").and_then(Value::as_u64);
    let host_page: Option<u64> = params.get("page").and_then(Value::as_u64);

    let effective_limit: u64 = host_page_size.or(parsed.limit).unwrap_or(100);
    let effective_offset: u64 = match host_page {
        Some(p) if p > 1 => (p - 1) * effective_limit,
        _ => parsed.offset.unwrap_or(0),
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

    if let Some(filter) = &parsed.where_clause {
        let firestore_filter = crate::firestore_filter::build_filter(filter);
        q = q.filter(move |_| Some(firestore_filter.clone()));
    }

    if !order_items.is_empty() {
        q = q.order_by(order_items.clone());
    }

    // Apply effective limit unconditionally.
    q = q.limit(effective_limit as u32);

    // Build query key for cursor and count caches.
    let query_key = crate::state::QueryKey {
        table: parsed.table.clone(),
        where_canonical: parsed
            .where_clause
            .as_ref()
            .map(canonical_filter)
            .unwrap_or_default(),
        order_by_canonical: canonical_order_by(&parsed.order_by),
    };

    // Look up a cached cursor for this offset (write lock required — TtlLruCache::get is &mut).
    let cursor_doc: Option<firestore::FirestoreDocument> = if effective_offset > 0 {
        crate::state::CURSOR_CACHE
            .write()
            .unwrap()
            .get(&query_key)
            .and_then(|entry| entry.cursors.get(&effective_offset).cloned())
    } else {
        None
    };

    if let Some(ref doc) = cursor_doc {
        // Build AfterValue cursor from the last document of the previous page.
        // Extract the ordered field values; fall back to the document reference
        // (__name__) when no ORDER BY was specified.
        let cursor_values: Vec<firestore::FirestoreValue> = if order_items.is_empty() {
            // No ORDER BY — Firestore implicitly orders by __name__ (the doc path).
            vec![firestore::FirestoreValue::from(
                gcloud_sdk::google::firestore::v1::Value {
                    value_type: Some(
                        gcloud_sdk::google::firestore::v1::value::ValueType::ReferenceValue(
                            doc.name.clone(),
                        ),
                    ),
                },
            )]
        } else {
            order_items
                .iter()
                .map(|(field, _dir)| {
                    let proto_val =
                        doc.fields.get(field).cloned().unwrap_or(
                            gcloud_sdk::google::firestore::v1::Value { value_type: None },
                        );
                    firestore::FirestoreValue::from(proto_val)
                })
                .collect()
        };
        q = q.start_at(firestore::FirestoreQueryCursor::AfterValue(cursor_values));
    } else if effective_offset > 0 {
        // No cursor cached — fall back to OFFSET (skip).
        q = q.offset(effective_offset as u32);
    }

    // Build cache key for count.
    let count_key = crate::state::CountKey {
        table: parsed.table.clone(),
        where_canonical: parsed
            .where_clause
            .as_ref()
            .map(canonical_filter)
            .unwrap_or_default(),
    };

    let cached_count: Option<u64> = crate::state::COUNT_CACHE
        .write()
        .unwrap()
        .get(&count_key)
        .copied();

    let started = std::time::Instant::now();

    let (docs_result, count_result) = if let Some(c) = cached_count {
        // Cache hit — only run the data query.
        let d = q.query().await;
        (d, Ok(Some(c)))
    } else {
        // Build a separate count query with the same filter (no order/limit/offset).
        let mut count_q = db.fluent().select().from(parsed.table.as_str());
        if let Some(filter) = &parsed.where_clause {
            let cf = crate::firestore_filter::build_filter(filter);
            count_q = count_q.filter(move |_| Some(cf.clone()));
        }
        let count_fut = count_q
            .aggregate(|a| a.fields([a.field("count").count()]))
            .query();
        let (docs_res, agg_res) = tokio::join!(q.query(), count_fut);
        // Extract the count integer from the aggregation result document.
        // Returns Ok(Some(n)) on success, Ok(None) if the shape is unexpected,
        // or Err if the Firestore RPC itself failed.
        let count_res: Result<Option<u64>, firestore::errors::FirestoreError> = match agg_res {
            Ok(docs) => {
                let n = docs
                    .first()
                    .and_then(|d| d.fields.get("count"))
                    .and_then(|v| {
                        if let Some(
                            gcloud_sdk::google::firestore::v1::value::ValueType::IntegerValue(n),
                        ) = &v.value_type
                        {
                            Some(*n as u64)
                        } else {
                            None
                        }
                    });
                Ok(n)
            }
            Err(e) => Err(e),
        };
        (docs_res, count_res)
    };

    let elapsed = started.elapsed().as_millis() as u64;

    let docs: Vec<firestore::FirestoreDocument> = match docs_result {
        Ok(d) => d,
        Err(e) => return error_from_query(id, &e),
    };

    // Update cursor cache: store the last doc as the cursor for the next page offset.
    if let Some(last_doc) = docs.last() {
        let next_offset = effective_offset + docs.len() as u64;
        let mut cache = crate::state::CURSOR_CACHE.write().unwrap();
        // Clone existing cursors (if any) before re-borrowing for insert.
        let existing_cursors = cache.get(&query_key).map(|e| e.cursors.clone());
        let mut cursors = existing_cursors.unwrap_or_default();
        cursors.insert(next_offset, last_doc.clone());
        cache.insert(query_key, crate::state::CursorEntry { cursors });
    }

    let total_count: u64 = match count_result {
        Ok(Some(n)) => {
            crate::state::COUNT_CACHE
                .write()
                .unwrap()
                .insert(count_key, n);
            n
        }
        Ok(None) => {
            // Firestore returned an aggregation result but without the expected
            // integer "count" field. Skip the cache to avoid a stale zero being
            // served to subsequent callers, and fall back to docs.len().
            eprintln!("count aggregation returned unexpected shape (falling back to rows.len)");
            docs.len() as u64
        }
        Err(e) => {
            // Count RPC failed. Non-fatal — fall back to docs.len().
            eprintln!("count failed (falling back to rows.len): {e}");
            docs.len() as u64
        }
    };

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
            crate::schema_infer::infer(&sample, &[])
        }
    };

    let column_names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let rows: Vec<Value> = docs.iter().map(|d| serialize_row(d, &columns)).collect();

    ok_response(
        id,
        json!({
            "columns": column_names,
            "rows": rows,
            "total_count": total_count,
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
        if col.name == crate::schema_infer::ID_COLUMN {
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
        Some(V::ArrayValue(a)) => Value::Array(a.values.iter().map(serialize_value).collect()),
        Some(V::MapValue(m)) => Value::Object(
            m.fields
                .iter()
                .map(|(k, x)| (k.clone(), serialize_value(x)))
                .collect(),
        ),
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

pub async fn explain_query(id: Value, params: &Value) -> Value {
    let sql = params
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut parsed = match crate::query_parser::parse(&sql) {
        Ok(p) => p,
        Err(e) => return crate::rpc::error_response(id, -32602, &e, None),
    };

    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    if let (Some(filter), Some(settings)) = (parsed.where_clause.as_mut(), crate::state::settings())
    {
        crate::firestore_filter::rewrite_doc_id(
            filter,
            &parsed.table,
            &settings.project_id,
            &settings.database_id,
        );
    }

    if let Some(filter) = &parsed.where_clause {
        if let Err(msg) = crate::firestore_filter::validate(filter) {
            return crate::rpc::error_response(id, -32602, &msg, None);
        }
    }

    let mut q = db.fluent().select().from(parsed.table.as_str());

    if let Some(filter) = &parsed.where_clause {
        let firestore_filter = crate::firestore_filter::build_filter(filter);
        q = q.filter(move |_| Some(firestore_filter.clone()));
    }

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

    if !order_items.is_empty() {
        q = q.order_by(order_items);
    }

    if let Some(n) = parsed.limit {
        q = q.limit(n as u32);
    }

    // Enable explain mode (sets explain_options on the query params).
    // The explain metrics are returned in the metadata of the last stream item.
    let started = std::time::Instant::now();
    let stream_result = q.explain().stream_query_with_metadata().await;
    let elapsed = started.elapsed().as_millis() as u64;

    let stream = match stream_result {
        Ok(s) => s,
        Err(e) => return error_from_query(id, &e),
    };

    use futures::TryStreamExt;
    let items: Vec<firestore::FirestoreWithMetadata<firestore::FirestoreDocument>> =
        match stream.try_collect().await {
            Ok(v) => v,
            Err(e) => return error_from_query(id, &e),
        };

    // The explain metrics live in the last item's metadata (Firestore streams
    // them in the terminal RunQueryResponse that has no document payload).
    let explain_metrics = items
        .last()
        .and_then(|item| item.metadata.explain_metrics.as_ref());

    // Build the plan_summary JSON: list of index descriptors.
    let indexes_used: Value = explain_metrics
        .and_then(|m| m.plan_summary.as_ref())
        .map(|ps| {
            let list: Vec<Value> = ps
                .indexes_used
                .iter()
                .map(|idx| {
                    let obj: serde_json::Map<String, Value> = idx
                        .fields
                        .iter()
                        .map(|(k, v)| (k.clone(), proto_value_to_json(v)))
                        .collect();
                    Value::Object(obj)
                })
                .collect();
            Value::Array(list)
        })
        .unwrap_or(Value::Array(vec![]));

    // Build execution_stats JSON.
    let (results_returned, execution_duration_ms, read_operations) =
        if let Some(stats) = explain_metrics.and_then(|m| m.execution_stats.as_ref()) {
            let duration_ms = stats
                .execution_duration
                .map(|d| d.num_milliseconds())
                .unwrap_or(0);
            (
                stats.results_returned as u64,
                duration_ms,
                stats.read_operations as u64,
            )
        } else {
            (0, 0, 0)
        };

    // Build a human-readable plan_text.
    let plan_text = format!(
        "table={} filter={} order_by=[{}] limit={:?} indexes_used={}",
        parsed.table,
        parsed
            .where_clause
            .as_ref()
            .map(canonical_filter)
            .unwrap_or_else(|| "(none)".to_string()),
        canonical_order_by(&parsed.order_by),
        parsed.limit,
        indexes_used,
    );

    crate::rpc::ok_response(
        id,
        json!({
            "plan_text": plan_text,
            "documents_returned": results_returned,
            "documents_scanned": read_operations,
            "index_used": !indexes_used.as_array().map(|a| a.is_empty()).unwrap_or(true),
            "indexes_used": indexes_used,
            "execution_duration_ms": execution_duration_ms,
            "elapsed_ms": elapsed,
        }),
    )
}

/// Convert a `prost_types::Value` to a `serde_json::Value` for JSON serialisation.
fn proto_value_to_json(v: &gcloud_sdk::prost_types::Value) -> Value {
    use gcloud_sdk::prost_types::value::Kind;
    match v.kind.as_ref() {
        Some(Kind::NullValue(_)) | None => Value::Null,
        Some(Kind::BoolValue(b)) => Value::Bool(*b),
        Some(Kind::NumberValue(n)) => json!(n),
        Some(Kind::StringValue(s)) => Value::String(s.clone()),
        Some(Kind::StructValue(sv)) => Value::Object(
            sv.fields
                .iter()
                .map(|(k, val)| (k.clone(), proto_value_to_json(val)))
                .collect(),
        ),
        Some(Kind::ListValue(lv)) => {
            Value::Array(lv.values.iter().map(proto_value_to_json).collect())
        }
    }
}

/// Stable canonical form of a FilterExpr for cache keys.
pub(crate) fn canonical_filter(expr: &crate::query_parser::FilterExpr) -> String {
    use crate::query_parser::FilterExpr as F;
    match expr {
        F::Compare { field, op, value } => {
            format!(
                "(cmp {} {:?} {})",
                field.join("."),
                op,
                canonical_literal(value)
            )
        }
        F::In {
            field,
            values,
            negated,
        } => {
            let mut vs: Vec<String> = values.iter().map(canonical_literal).collect();
            vs.sort();
            format!(
                "({} {} [{}])",
                if *negated { "not_in" } else { "in" },
                field.join("."),
                vs.join(",")
            )
        }
        F::ArrayContains { field, value } => {
            format!("(ac {} {})", field.join("."), canonical_literal(value))
        }
        F::ArrayContainsAny { field, values } => {
            let mut vs: Vec<String> = values.iter().map(canonical_literal).collect();
            vs.sort();
            format!("(aca {} [{}])", field.join("."), vs.join(","))
        }
        F::And(children) => {
            let mut parts: Vec<String> = children.iter().map(canonical_filter).collect();
            parts.sort();
            format!("(and {})", parts.join(" "))
        }
        F::Or(children) => {
            let mut parts: Vec<String> = children.iter().map(canonical_filter).collect();
            parts.sort();
            format!("(or {})", parts.join(" "))
        }
    }
}

pub(crate) fn canonical_literal(lit: &crate::query_parser::Literal) -> String {
    use crate::query_parser::Literal as L;
    match lit {
        L::Str(s) => format!("'{s}'"),
        L::Int(n) => n.to_string(),
        L::Float(f) => format!("{f:?}"),
        L::Bool(b) => b.to_string(),
        L::Null => "null".to_string(),
        L::Timestamp(dt) => format!("ts:{}", dt.to_rfc3339()),
        L::Reference(p) => format!("ref:{p}"),
    }
}

#[allow(dead_code)]
pub(crate) fn canonical_order_by(items: &[crate::query_parser::OrderItem]) -> String {
    items
        .iter()
        .map(|i| format!("{} {}", i.field, if i.desc { "DESC" } else { "ASC" }))
        .collect::<Vec<_>>()
        .join(", ")
}
