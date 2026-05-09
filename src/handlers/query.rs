//! Connection and query execution.

use serde_json::{json, Value};

use crate::rpc::ok_response;

pub async fn initialize(id: Value, params: &Value) -> Value {
    let settings_value = params.get("settings").cloned().unwrap_or(Value::Null);
    let settings = crate::models::Settings::from_value(&settings_value);

    // Load optional schema overrides for this (project, database). A bad file
    // (missing dir, invalid JSON, unknown type override) MUST surface here so
    // the user sees the failure during connect, not later in get_columns.
    let overrides = match crate::schema_overrides::load(
        settings.schema_overrides_dir.as_deref(),
        &settings.project_id,
        &settings.database_id,
    ) {
        Ok(o) => o,
        Err(e) => return crate::rpc::error_response(id, -32602, &e, None),
    };

    let _ = crate::state::SETTINGS.set(settings); // second initialize is a no-op
    let _ = crate::state::SCHEMA_OVERRIDES.set(overrides);
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

    if let Err(resp) = prepare_filter(&mut parsed, &id) {
        return resp;
    }

    // Resolve effective limit/offset: params.page/page_size override SQL.
    let host_page_size: Option<u64> = params.get("page_size").and_then(Value::as_u64);
    let host_page: Option<u64> = params.get("page").and_then(Value::as_u64);

    let effective_limit: u64 = host_page_size.or(parsed.limit).unwrap_or(100);
    let effective_offset: u64 = match host_page {
        Some(p) if p > 1 => (p - 1) * effective_limit,
        _ => parsed.offset.unwrap_or(0),
    };

    let db = match crate::client::resolve(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    let order_items = to_firestore_order(&parsed.order_by);

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

    // Look up a cached cursor for this offset. If the exact offset isn't
    // cached, fall back to the largest cursor ≤ offset and apply the remainder
    // as a Firestore-side OFFSET. This is still cheaper than scanning from
    // zero, as long as the user paginates roughly forward.
    let (cursor_doc, residual_offset): (Option<firestore::FirestoreDocument>, u64) =
        if effective_offset > 0 {
            let mut cache = crate::state::lock_cursor_cache();
            match cache.get(&query_key) {
                Some(entry) => {
                    if let Some(doc) = entry.cursors.get(&effective_offset) {
                        (Some(doc.clone()), 0)
                    } else if let Some((cursor_off, doc)) =
                        entry.cursors.range(..effective_offset).next_back()
                    {
                        (Some(doc.clone()), effective_offset - cursor_off)
                    } else {
                        (None, effective_offset)
                    }
                }
                None => (None, effective_offset),
            }
        } else {
            (None, 0)
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
        if residual_offset > 0 {
            // Cursor was approximate — apply the remaining gap as OFFSET.
            q = q.offset(residual_offset.min(u32::MAX as u64) as u32);
        }
    } else if effective_offset > 0 {
        // No cursor cached — fall back to OFFSET (skip).
        q = q.offset(effective_offset.min(u32::MAX as u64) as u32);
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

    let cached_count: Option<u64> = crate::state::lock_count_cache()
        .get(&count_key)
        .copied();

    let started = std::time::Instant::now();

    // Run a fresh COUNT only when (a) we don't have a cached value AND (b) the
    // caller is on page 1 (offset == 0). For later pages the total hasn't
    // changed since page 1; running an unconditional aggregation would double
    // the Firestore reads with no observable benefit.
    let should_count = cached_count.is_none() && effective_offset == 0;

    let (docs_result, count_result) = if let Some(c) = cached_count {
        let d = q.query().await;
        (d, Ok(Some(c)))
    } else if !should_count {
        // Page 2+ with cold cache: skip count entirely, fall back to rows.len()
        // when assembling total_count below.
        let d = q.query().await;
        (d, Ok(None))
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
        // Ok(Some(n)) on success, Ok(None) if the shape is unexpected,
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
        let mut cache = crate::state::lock_cursor_cache();
        // Clone existing cursors (if any) before re-borrowing for insert.
        let existing_cursors = cache.get(&query_key).map(|e| e.cursors.clone());
        let mut cursors = existing_cursors.unwrap_or_default();
        cursors.insert(next_offset, last_doc.clone());
        cache.insert(query_key, crate::state::CursorEntry { cursors });
    }

    let total_count: u64 = match count_result {
        Ok(Some(n)) => {
            crate::state::lock_count_cache().insert(count_key, n);
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

    let columns = match crate::state::schema_cache_read().get(&parsed.table) {
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

    // Client-side projection: Firestore has no server-side field selection,
    // so we filter the columns + slice each row after the docs come back.
    let (column_names, rows) = match &parsed.columns {
        Some(requested) => project_columns(&column_names, &rows, requested),
        None => (column_names, rows),
    };

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

/// Filter the result set to only the columns named in the SELECT clause.
/// Requested columns missing from the inferred schema are kept in the output
/// with `null` values — schema inference is sample-based, so a column the user
/// asks for may be absent in the sample but present in some documents.
fn project_columns(
    all_names: &[String],
    all_rows: &[Value],
    requested: &[String],
) -> (Vec<String>, Vec<Value>) {
    let indices: Vec<Option<usize>> = requested
        .iter()
        .map(|r| all_names.iter().position(|n| n == r))
        .collect();
    let rows: Vec<Value> = all_rows
        .iter()
        .map(|row| {
            let arr = row.as_array().cloned().unwrap_or_default();
            let projected: Vec<Value> = indices
                .iter()
                .map(|idx| {
                    idx.and_then(|i| arr.get(i).cloned())
                        .unwrap_or(Value::Null)
                })
                .collect();
            Value::Array(projected)
        })
        .collect();
    (requested.to_vec(), rows)
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

/// Apply the doc-id rewrite + Firestore restriction validation. Returns a
/// JSON-RPC error response if validation fails. Used by both `execute_query`
/// and `explain_query` so the same WHERE clause behaviour is enforced in both.
fn prepare_filter(
    parsed: &mut crate::query_parser::ParsedQuery,
    id: &Value,
) -> Result<(), Value> {
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
            return Err(crate::rpc::error_response(id.clone(), -32602, &msg, None));
        }
    }
    Ok(())
}

/// Map our internal `OrderItem`s to the firestore-rs direction enum.
fn to_firestore_order(
    items: &[crate::query_parser::OrderItem],
) -> Vec<(String, firestore::FirestoreQueryDirection)> {
    items
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
        .collect()
}

pub async fn explain_query(id: Value, params: &Value) -> Value {
    let sql = params
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let analyze = params.get("analyze").and_then(Value::as_bool).unwrap_or(false);
    let mut parsed = match crate::query_parser::parse(&sql) {
        Ok(p) => p,
        Err(e) => return crate::rpc::error_response(id, -32602, &e, None),
    };

    if let Err(resp) = prepare_filter(&mut parsed, &id) {
        return resp;
    }

    let db = match crate::client::resolve(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    let mut q = db.fluent().select().from(parsed.table.as_str());

    if let Some(filter) = &parsed.where_clause {
        let firestore_filter = crate::firestore_filter::build_filter(filter);
        q = q.filter(move |_| Some(firestore_filter.clone()));
    }

    let order_items = to_firestore_order(&parsed.order_by);
    if !order_items.is_empty() {
        q = q.order_by(order_items);
    }

    if let Some(n) = parsed.limit {
        q = q.limit(n as u32);
    }

    // analyze=true tells Firestore to actually run the query and return
    // execution stats; false returns the plan only.
    let explain_opts = firestore::FirestoreExplainOptions::new().with_analyze(analyze);
    let started = std::time::Instant::now();
    let stream_result = q
        .explain_with_options(explain_opts)
        .stream_query_with_metadata()
        .await;
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

    // Tabularis (src/types/explain.ts) expects a single-root tree:
    //   ExplainPlan { root: ExplainNode, planning_time_ms, execution_time_ms,
    //                 original_query, driver, has_analyze_data, raw_output }
    // Firestore's plan is a flat blob of stats, not a tree, so we emit one
    // root node carrying the table as `relation`, the result count as
    // `actual_rows`, the duration as `actual_time_ms`, and stuff the
    // Firestore-specific stats (read_operations, indexes_used) in `extra`
    // for the visualizer to surface.
    let has_analyze = explain_metrics
        .and_then(|m| m.execution_stats.as_ref())
        .is_some();

    let mut extra = serde_json::Map::new();
    extra.insert("documents_scanned".into(), json!(read_operations));
    extra.insert(
        "index_used".into(),
        json!(!indexes_used
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)),
    );
    extra.insert("indexes_used".into(), indexes_used);
    if let Some(filter) = parsed.where_clause.as_ref() {
        extra.insert("filter_canonical".into(), json!(canonical_filter(filter)));
    }
    if !parsed.order_by.is_empty() {
        extra.insert("order_by".into(), json!(canonical_order_by(&parsed.order_by)));
    }
    if let Some(n) = parsed.limit {
        extra.insert("limit".into(), json!(n));
    }

    let root = json!({
        "id": "firestore-root",
        "node_type": "Firestore Query",
        "relation": parsed.table,
        "startup_cost": Value::Null,
        "total_cost": Value::Null,
        "plan_rows": Value::Null,
        "actual_rows": if has_analyze { json!(results_returned) } else { Value::Null },
        "actual_time_ms": if has_analyze { json!(execution_duration_ms) } else { Value::Null },
        "actual_loops": Value::Null,
        "buffers_hit": Value::Null,
        "buffers_read": Value::Null,
        "filter": Value::Null,
        "index_condition": Value::Null,
        "join_type": Value::Null,
        "hash_condition": Value::Null,
        "extra": Value::Object(extra),
        "children": Value::Array(vec![]),
    });

    crate::rpc::ok_response(
        id,
        json!({
            "root": root,
            "planning_time_ms": Value::Null,
            "execution_time_ms": if has_analyze { json!(execution_duration_ms) } else { Value::Null },
            "original_query": sql,
            "driver": "firestore",
            "has_analyze_data": has_analyze,
            "raw_output": Value::Null,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_parser::{CmpOp, FilterExpr, Literal, OrderItem};
    use crate::schema_infer::ColumnInfo;
    use gcloud_sdk::google::firestore::v1::value::ValueType;
    use gcloud_sdk::google::firestore::v1::{
        ArrayValue, MapValue, Value as ProtoValue,
    };

    fn proto_str(s: &str) -> ProtoValue {
        ProtoValue {
            value_type: Some(ValueType::StringValue(s.into())),
        }
    }

    fn proto_int(n: i64) -> ProtoValue {
        ProtoValue {
            value_type: Some(ValueType::IntegerValue(n)),
        }
    }

    fn col(name: &str, data_type: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            data_type: data_type.into(),
            is_nullable: true,
            references: None,
            comment: None,
        }
    }

    #[test]
    fn doc_short_id_takes_last_segment() {
        let doc = firestore::FirestoreDocument {
            name: "projects/p/databases/(default)/documents/users/abc123".into(),
            fields: std::collections::HashMap::new(),
            create_time: None,
            update_time: None,
        };
        assert_eq!(doc_short_id(&doc), "abc123");
    }

    #[test]
    fn doc_short_id_handles_empty_name() {
        let doc = firestore::FirestoreDocument {
            name: String::new(),
            fields: std::collections::HashMap::new(),
            create_time: None,
            update_time: None,
        };
        assert_eq!(doc_short_id(&doc), "");
    }

    #[test]
    fn serialize_value_null() {
        let v = ProtoValue {
            value_type: Some(ValueType::NullValue(0)),
        };
        assert_eq!(serialize_value(&v), Value::Null);
    }

    #[test]
    fn serialize_value_bool_int_double_string() {
        assert_eq!(
            serialize_value(&ProtoValue {
                value_type: Some(ValueType::BooleanValue(true)),
            }),
            Value::Bool(true)
        );
        assert_eq!(serialize_value(&proto_int(42)), json!(42));
        assert_eq!(
            serialize_value(&ProtoValue {
                value_type: Some(ValueType::DoubleValue(2.5)),
            }),
            json!(2.5)
        );
        assert_eq!(serialize_value(&proto_str("hello")), json!("hello"));
    }

    #[test]
    fn serialize_value_timestamp_emits_rfc3339() {
        let v = ProtoValue {
            value_type: Some(ValueType::TimestampValue(
                gcloud_sdk::prost_types::Timestamp {
                    seconds: 1_778_322_600,
                    nanos: 0,
                },
            )),
        };
        let s = serialize_value(&v);
        assert!(s.as_str().unwrap().starts_with("2026-05-09T10:30:00"));
    }

    #[test]
    fn serialize_value_array_emits_json_string() {
        let v = ProtoValue {
            value_type: Some(ValueType::ArrayValue(ArrayValue {
                values: vec![proto_str("a"), proto_str("b")],
            })),
        };
        // Phase-2 quirk: arrays are JSON-stringified for Tabularis grid hover.
        assert_eq!(serialize_value(&v), json!("[\"a\",\"b\"]"));
    }

    #[test]
    fn serialize_value_map_emits_json_string() {
        let mut fields = std::collections::HashMap::new();
        fields.insert("k".to_string(), proto_int(7));
        let v = ProtoValue {
            value_type: Some(ValueType::MapValue(MapValue { fields })),
        };
        assert_eq!(serialize_value(&v), json!("{\"k\":7}"));
    }

    #[test]
    fn serialize_value_reference_passes_through() {
        let v = ProtoValue {
            value_type: Some(ValueType::ReferenceValue(
                "projects/p/databases/(default)/documents/users/abc".into(),
            )),
        };
        assert_eq!(
            serialize_value(&v),
            json!("projects/p/databases/(default)/documents/users/abc")
        );
    }

    #[test]
    fn to_firestore_order_maps_directions() {
        let items = vec![
            OrderItem {
                field: "createdAt".into(),
                desc: true,
            },
            OrderItem {
                field: "name".into(),
                desc: false,
            },
        ];
        let mapped = to_firestore_order(&items);
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].0, "createdAt");
        assert_eq!(mapped[1].0, "name");
        assert!(matches!(
            mapped[0].1,
            firestore::FirestoreQueryDirection::Descending
        ));
        assert!(matches!(
            mapped[1].1,
            firestore::FirestoreQueryDirection::Ascending
        ));
    }

    #[test]
    fn to_firestore_order_empty_input() {
        assert!(to_firestore_order(&[]).is_empty());
    }

    #[test]
    fn project_columns_keeps_only_requested() {
        let names = vec!["id".into(), "email".into(), "rating".into()];
        let rows = vec![json!(["abc", "x@y.de", 5])];
        let (cols, rows) = project_columns(&names, &rows, &["email".into()]);
        assert_eq!(cols, vec!["email"]);
        assert_eq!(rows, vec![json!(["x@y.de"])]);
    }

    #[test]
    fn project_columns_keeps_unknown_with_null_fill() {
        let names = vec!["id".into(), "email".into()];
        let rows = vec![json!(["abc", "x@y.de"])];
        let (cols, rows) = project_columns(&names, &rows, &["id".into(), "missing".into()]);
        assert_eq!(cols, vec!["id", "missing"]);
        assert_eq!(rows, vec![json!(["abc", null])]);
    }

    #[test]
    fn project_columns_preserves_order() {
        let names = vec!["a".into(), "b".into(), "c".into()];
        let rows = vec![json!([1, 2, 3])];
        let (cols, rows) = project_columns(&names, &rows, &["c".into(), "a".into()]);
        assert_eq!(cols, vec!["c", "a"]);
        assert_eq!(rows, vec![json!([3, 1])]);
    }

    #[test]
    fn canonical_order_by_formats() {
        let items = vec![
            OrderItem {
                field: "x".into(),
                desc: false,
            },
            OrderItem {
                field: "y".into(),
                desc: true,
            },
        ];
        assert_eq!(canonical_order_by(&items), "x ASC, y DESC");
    }

    #[test]
    fn canonical_order_by_empty() {
        assert_eq!(canonical_order_by(&[]), "");
    }

    #[test]
    fn canonical_filter_eq() {
        let f = FilterExpr::Compare {
            field: vec!["status".into()],
            op: CmpOp::Eq,
            value: Literal::Str("active".into()),
        };
        let s = canonical_filter(&f);
        assert!(s.contains("status"));
        assert!(s.contains("active"));
    }

    #[test]
    fn canonical_filter_and_is_deterministic() {
        // Same logical expression in different argument orders should canonicalise
        // to the same string — that's what makes it usable as a cache key.
        let f1 = FilterExpr::And(vec![
            FilterExpr::Compare {
                field: vec!["a".into()],
                op: CmpOp::Eq,
                value: Literal::Int(1),
            },
            FilterExpr::Compare {
                field: vec!["b".into()],
                op: CmpOp::Eq,
                value: Literal::Int(2),
            },
        ]);
        let f2 = FilterExpr::And(vec![
            FilterExpr::Compare {
                field: vec!["b".into()],
                op: CmpOp::Eq,
                value: Literal::Int(2),
            },
            FilterExpr::Compare {
                field: vec!["a".into()],
                op: CmpOp::Eq,
                value: Literal::Int(1),
            },
        ]);
        assert_eq!(canonical_filter(&f1), canonical_filter(&f2));
    }

    #[test]
    fn serialize_row_includes_id_first() {
        let cols = vec![col("id", "string"), col("email", "string")];
        let mut fields = std::collections::HashMap::new();
        fields.insert("email".into(), proto_str("x@y.de"));
        let doc = firestore::FirestoreDocument {
            name: "projects/p/databases/(default)/documents/test/abc".into(),
            fields,
            create_time: None,
            update_time: None,
        };
        let row = serialize_row(&doc, &cols);
        assert_eq!(row, json!(["abc", "x@y.de"]));
    }

    #[test]
    fn serialize_row_fills_missing_with_null() {
        let cols = vec![col("id", "string"), col("missing", "string")];
        let doc = firestore::FirestoreDocument {
            name: "projects/p/databases/(default)/documents/test/abc".into(),
            fields: std::collections::HashMap::new(),
            create_time: None,
            update_time: None,
        };
        let row = serialize_row(&doc, &cols);
        assert_eq!(row, json!(["abc", null]));
    }
}
