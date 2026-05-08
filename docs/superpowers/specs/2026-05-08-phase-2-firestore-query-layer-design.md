# Phase 2 — Firestore Query Layer

**Status:** Draft, ready for implementation planning
**Date:** 2026-05-08
**Scope:** Second sub-project. Builds on shipped Phase 1 (read-only viewer with `SELECT *` only). Phase 2 turns the plugin into a daily driver by adding WHERE filtering, OR conjunction, cursor pagination, real `COUNT(*)`, native nested JSON, an ER diagram, and expanded error mapping.

## Goal

A Tabularis user opens the firestore plugin's data grid against a real Firestore project and can:

1. Filter rows with the full Firestore filter set: `=`, `!=`, `<`, `<=`, `>`, `>=`, `IN`, `NOT IN`, `ARRAY_CONTAINS`, `ARRAY_CONTAINS_ANY`.
2. Combine filters with both `AND` and `OR`, including parenthesized grouping (`(a OR b) AND c`).
3. Filter on nested-map fields via dot notation: `WHERE address.country = 'DE'`.
4. Paginate large result sets with constant per-page latency: sequential next-page clicks use Firestore cursors, jump-to-page falls back to OFFSET.
5. See the real total row count in the grid footer (`1–100 of 5247`), not just the current page size.
6. View a map/array column value as expandable nested JSON (or, if Tabularis can't render it, as the same stringified JSON we shipped in Phase 1 — decided by a smoke-test gate).
7. Open Tabularis' ER-diagram view and see Reference-typed fields as inferred foreign-key edges between collections.
8. Get actionable error toasts for the common Firestore failure modes (permission denied with IAM hint, quota exceeded, deadline, transient unavailability, missing-index URL — Phase 1 carry-over).

## Non-goals (Phase 3 / Phase 4)

- INSERT/UPDATE/DELETE — Phase 3.
- DDL generators — likely permanently `not_implemented`.
- Subcollection navigation — Phase 4 (UI extension).
- Multi-database per connection — Phase 4 (state-shape change).
- Multi-project per connection via `connection-modal.connection_content` UI extension — Phase 4.
- Auth wizard / OAuth flow — Phase 4.
- Real-time listener (`watch`) — Phase 4 or 5.

## Architecture overview

Phase 1 left us with three pure-logic modules (`query_parser`, `schema_infer`, `firestore_error`), an async dispatch loop, lazy `FirestoreDb` initialisation behind `tokio::sync::OnceCell`, and a per-collection schema cache. Phase 2 extends the parser to a boolean-tree AST, adds two more global caches (cursor cache, count cache), wires `firestore-rs` composite filters, and replaces the stringified map/array serialisation with native JSON values.

New files:
- `src/cache.rs` — generic TTL+LRU cache used for both cursor and count caches
- `src/firestore_filter.rs` — `FilterExpr` AST → `firestore::FirestoreQueryFilter` mapper, plus pre-flight Firestore-restriction validation

Substantially modified:
- `src/query_parser.rs` — boolean-tree grammar, new tokens, new AST nodes
- `src/handlers/query.rs` — `execute_query` now consumes parsed `WHERE`, runs count + data in parallel, manages cursor cache; `explain_query` is real
- `src/handlers/metadata.rs` — `get_schema_snapshot` populated with FK relationships
- `src/schema_infer.rs` — `ColumnInfo.references: Option<String>` added; reference-target extraction from doc paths; `serialize_value` switches Map/Array to native JSON
- `src/firestore_error.rs` — four new `ErrorKind` variants with hint messages
- `src/state.rs` — `CURSOR_CACHE`, `COUNT_CACHE` globals

## Grammar extensions

### New tokens

| Token | Lexed from |
|---|---|
| `Op(CmpOp)` | `=`, `==`, `!=`, `<>`, `<`, `<=`, `>`, `>=` |
| `LParen` / `RParen` | `(`, `)` |
| `StringLiteral(String)` | `'...'` (single quotes), `\'` escape inside |
| `Word("AND" / "OR" / "NOT" / "IN" / "TRUE" / "FALSE" / "NULL")` | already tokenises as `Word`; matched case-insensitively at parse time |

The Phase 1 catch-all `Symbol(char)` shrinks: `<`, `>`, `(`, `)` graduate to first-class tokens. Other punctuation still falls through to `Symbol`.

`==` (double-equal) is accepted as a synonym for `=` because Firestore-people instinctively type `==`.

### AST shape

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    pub table: String,
    pub where_clause: Option<FilterExpr>,
    pub order_by: Vec<OrderItem>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterExpr {
    Compare { field: Vec<String>, op: CmpOp, value: Literal },
    In { field: Vec<String>, values: Vec<Literal>, negated: bool },
    ArrayContains { field: Vec<String>, value: Literal },
    ArrayContainsAny { field: Vec<String>, values: Vec<Literal> },
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp { Eq, Ne, Lt, Le, Gt, Ge }

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    Timestamp(chrono::DateTime<chrono::Utc>),
}
```

Field paths are stored as `Vec<String>` so dot notation (`address.city`) survives without re-splitting at the mapper.

### Precedence

Standard SQL: `NOT > AND > OR`. The recursive-descent parser splits `parse_or` → `parse_and` → `parse_not` → `parse_atom`. Atoms are either a `Compare` / `In` / `ArrayContains*` invocation, or a parenthesised `parse_or` for grouping.

### Accepted query forms

```sql
SELECT * FROM "customers" WHERE email = 'alice@x.de'
SELECT * FROM "events"    WHERE ts > '2026-01-01' ORDER BY ts DESC LIMIT 100
SELECT * FROM "products"  WHERE category IN ('books', 'media') AND price > 10
SELECT * FROM "posts"     WHERE ARRAY_CONTAINS(tags, 'urgent')
SELECT * FROM "tickets"   WHERE ARRAY_CONTAINS_ANY(tags, ('p0', 'p1'))
SELECT * FROM "users"     WHERE (region = 'eu' OR region = 'us') AND active = TRUE
SELECT * FROM "orders"    WHERE address.country = 'DE' AND total > 100
SELECT * FROM "logs"      WHERE level <> 'debug' AND service = 'api'
```

### Type coercion

| Source | Literal type |
|---|---|
| `42` | `Int` |
| `3.14` | `Float` |
| `'foo'` | `Str` |
| `TRUE` / `FALSE` (case-insensitive) | `Bool` |
| `NULL` (case-insensitive) | `Null` |
| `TIMESTAMP '2026-01-01T00:00:00Z'` | `Timestamp(chrono::DateTime<Utc>)` — parser recognises the `TIMESTAMP` keyword before the string literal, parses the inner string as RFC 3339, fails parse on malformed input |

Plain `'2026-01-01'` stays a string literal — auto-detection of ISO-8601 was rejected as too fragile (e.g., `'42'` looks like a number but the user clearly meant string).

### Negative grammar

These produce explicit Phase-2 errors (not silent fallthrough):
- Bare `WHERE` with no expression → `"WHERE clause is empty"`
- `WHERE x =` (missing right operand) → `"expected literal after '='"`
- Unbalanced parens → `"unbalanced parenthesis"`
- `IN ()` (empty value list) → `"IN/NOT IN requires at least one value"`
- `ARRAY_CONTAINS_ANY(field, value)` (single value not in tuple) → `"ARRAY_CONTAINS_ANY needs a (...) value list"`
- Unknown function: `WHERE FOO_BAR(x)` → `"unknown function 'FOO_BAR' (did you mean ARRAY_CONTAINS or ARRAY_CONTAINS_ANY?)"`

## Firestore mapping (`firestore_filter.rs`)

A pure function `fn build_filter(expr: &FilterExpr) -> FirestoreQueryFilter` walks the AST and produces the firestore-rs filter tree. The exact API names (`FirestoreQueryFilterCompare::Equal`, `FirestoreQueryFilter::Composite`, etc.) are verified at implementation time against firestore 0.48.0's rustdoc — historically the crate has small renames between minor versions.

### Pre-flight validation

Before the build_filter call hits the wire, a separate `fn validate(expr: &FilterExpr) -> Result<(), String>` checks Firestore's compound-filter restrictions:

| Restriction | Error message |
|---|---|
| Inequality (`<`, `<=`, `>`, `>=`, `!=`) on > 1 distinct field per query | `"Firestore allows inequality on at most one field per query (saw <fields>). Adjust the filter or split into multiple queries."` |
| `IN` / `NOT IN` / `ARRAY_CONTAINS_ANY` value list with > 30 entries | `"Firestore limits IN / NOT IN / ARRAY_CONTAINS_ANY to 30 values per query (saw <n>)."` |
| Both `ARRAY_CONTAINS` and `ARRAY_CONTAINS_ANY` in same query | `"Firestore disallows ARRAY_CONTAINS and ARRAY_CONTAINS_ANY in the same query."` |
| > 1 `ARRAY_CONTAINS` on the same field | `"Firestore allows at most one ARRAY_CONTAINS per field."` |

Validation walks the AST once (depth-first), accumulating the per-field operator usage. JSON-RPC error code `-32602` for all validation failures.

We catch these before the wire because Firestore's own error messages for these cases are cryptic; surfacing the real rule in plain English is a daily-driver UX win.

## Pagination — hybrid cursor + OFFSET

### Inputs

`execute_query` reads three sources of pagination intent and resolves them in priority order:

1. `params.page_size` and `params.page` (host-driven; what Tabularis sends when the user clicks pages)
2. `LIMIT n` / `OFFSET o` from the parsed SQL
3. Defaults: `limit = 100`, `offset = 0`

If both are present, the host values win — Tabularis is actively paginating; the SQL is a template.

### Cursor cache

```rust
// state.rs
pub static CURSOR_CACHE: Lazy<RwLock<TtlLruCache<QueryKey, CursorEntry>>> = …;

#[derive(Hash, Eq, PartialEq, Clone)]
struct QueryKey {
    table: String,
    where_canonical: String,    // normalised string repr of FilterExpr
    order_by_canonical: String, // normalised string repr of order_by Vec
}

struct CursorEntry {
    /// Map from page-end offset to the FirestoreDocument that closes that page.
    /// Used as start_after() target for the next sequential page.
    cursors: BTreeMap<u64, firestore::FirestoreDocument>,
}
```

### Decision logic

```
Given effective (offset, limit) and the parsed query's QueryKey:

if cursor_cache[key].cursors[offset] exists:
    use start_after(cursor_cache[key].cursors[offset]).limit(limit)
elif offset == 0:
    use limit(limit)                                    # first page, no cursor needed
else:
    use limit(limit).offset(offset)                     # jump-to-page; expensive but correct

after the query succeeds and we have docs:
    cursor_cache[key].cursors[offset + docs.len()] = docs.last().unwrap()
```

This means: sequential pagination (1→2→3→…) hits the cursor path after page 1 and stays at constant latency. Jump-to-page falls back to OFFSET (Phase 1 behaviour). User experience is monotonically better than Phase 1, never worse.

### Eviction

`TtlLruCache` evicts on two axes:
- LRU bound: 100 distinct query keys; oldest evicted on overflow
- TTL: 5 minutes per entry; expired entries lazy-removed on next access

Phase 3 will additionally invalidate per-table on writes.

### Canonicalisation

`where_canonical` is built by serialising the `FilterExpr` AST in a stable, sorted order:
- `And([a, b])` and `And([b, a])` produce the same string (children sorted by their canonical form)
- `Compare` literals printed in a consistent format (string with single quotes, int as decimal, etc.)

This means semantically equivalent queries hit the same cache entry even if the user types them differently between sessions.

## COUNT(*) — cache pro (table, WHERE)

```rust
// state.rs
pub static COUNT_CACHE: Lazy<RwLock<TtlLruCache<CountKey, u64>>> = …;

#[derive(Hash, Eq, PartialEq, Clone)]
struct CountKey {
    table: String,
    where_canonical: String,    // ORDER BY / LIMIT / OFFSET excluded — they don't affect total
}
```

### Flow inside `execute_query`

```
1. Parse query → ParsedQuery { ... }
2. Build CountKey
3. cache_lookup → Option<u64>
4. Build the data query (with limit/offset/cursor as above)
5. If cache_lookup is Some(n):
       data_docs = data_query.await
       total = n
   else:
       (data_docs, count_result) = tokio::join!(
           data_query,
           db.fluent().select().from(table).filter(<built_filter>).count()
       )
       match count_result:
           Ok(n)  → cache.insert(CountKey, n); total = n
           Err(_) → total = data_docs.len() as u64    // graceful fallback, log the count error
6. Return { columns, rows, total_count: total, affected_rows: 0, execution_time_ms: ... }
```

Two parallel awaits via `tokio::join!` mean the count latency is hidden behind the data latency, not added to it. First-page cost is `max(data_latency, count_latency)` instead of `data_latency + count_latency`.

### TTL

30 seconds. Same `TtlLruCache` as cursor cache; size bound 200 keys.

Trade-off: stale counts for up to 30s after an external write. Acceptable for a viewer; Phase 3 invalidates per-table on local writes.

### Failure handling

If the count call fails (e.g., a missing index for the COUNT-aggregation), we log the error to the plugin's stderr and return `total_count = rows.len()` for this call. The data query still succeeds; the user sees the page but a slightly underspecified total. No user-facing error toast — counts are best-effort.

## Native JSON for maps and arrays

`schema_infer::serialize_value` switches its `Map` and `Array` arms from `Value::String(serde_json::to_string(...))` to native `Value::Object(...)` / `Value::Array(...)`.

The `data_type` strings in `ColumnInfo` (returned by `get_columns`) stay `"map"` and `"array"` — the type system doesn't change, only the row-payload encoding.

### Smoke-test gate

This is the one Phase-2 decision that depends on host behaviour we can't verify by reading source. Before merging Phase 2:

1. Cherry-pick just the `serialize_value` Map/Array arms onto a spike branch
2. `just dev-install`, restart Tabularis
3. Open a collection with map fields (e.g., `customers.address`) and an array field (e.g., `users.favoriteAdvisorIds`)
4. Observe how Tabularis renders the cell

Outcomes:
- Tabularis renders nested JSON sanely (expandable tree, JSON-pretty preview, or even just stringified-on-display) → keep the native-JSON change
- Tabularis crashes / renders `[object Object]` / shows blank → revert this part of Phase 2; ship everything else; flag for Phase 4 UI extension

The test is fast (5 minutes) and the rollback is mechanical, so the spike-and-decide approach beats over-engineering a feature flag.

## explain_query

Real implementation calling Firestore's explain API:

```rust
pub async fn explain_query(id: Value, params: &Value) -> Value {
    let sql = params.get("query").and_then(Value::as_str).unwrap_or("").to_string();
    let parsed = match crate::query_parser::parse(&sql) {
        Ok(p) => p, Err(e) => return error_response(id, -32602, &e, None),
    };
    let db = match resolve_client(id.clone()).await { Ok(db) => db, Err(r) => return r };

    // Build the same Firestore query as execute_query would, but call .explain() instead of .query()
    let q = build_data_query(db, &parsed);
    match q.explain().await {
        Ok(plan) => ok_response(id, json!({
            "plan_text": format!("{plan:#?}"),
            "documents_returned": plan.execution_stats.docs_returned,
            "documents_scanned": plan.execution_stats.docs_scanned,
            "index_used": plan.execution_stats.index_used,
            "execution_duration_ms": plan.execution_stats.duration_ms,
        })),
        Err(e) => error_from(id, &e),
    }
}
```

Exact field names from `firestore::ExplainResults` confirmed at implementation time. Output shape is informational — Tabularis renders the JSON object directly in the explain panel.

## ER diagram (`get_schema_snapshot`)

`ColumnInfo` gains a `references: Option<String>` field. During schema inference (already running for `get_columns` and `get_all_columns_batch`), reference-typed values in sample documents have a known shape:

```
projects/<project>/databases/<database>/documents/<collection>/<docId>[/<sub>/<docId>]*
```

The collection is the segment immediately after `documents/`. We extract it and store as the `references` target.

If the same field has reference values pointing to multiple distinct collections across the sample (unusual but possible in dynamic-typing land), we mark `references = None` and treat the field as a plain string — multi-target references can't be a single foreign key.

`get_schema_snapshot` then runs the same parallel-fetch logic as `get_all_columns_batch` (which already exists from Phase 1), and assembles:

```json
{
  "tables": [{ "name": "...", "schema": null, "comment": null }, ...],
  "columns": { "<table>": [<column infos>], ... },
  "foreign_keys": {
    "<table>": [
      { "from_column": "customer_ref", "to_table": "customers", "to_column": "__id__" },
      ...
    ]
  }
}
```

`to_column` is always `__id__` because Firestore references always point to a document by its ID.

### Limitations

- Sparse reference fields (only present in a small fraction of docs) may be missed by sampling. Phase 4 can offer a "deep scan" mode or accept a config file with explicit FK declarations.
- Cross-database references are unsupported in the FK graph (we only model targets within the same database).

## Expanded error mapping

Four new `ErrorKind` variants with hint messages:

```rust
pub enum ErrorKind {
    FailedPrecondition,
    Unauthenticated,
    NotFound,
    PermissionDenied,    // NEW
    ResourceExhausted,   // NEW
    DeadlineExceeded,    // NEW
    Unavailable,         // NEW
    Other,
}
```

| Variant | Code | Message |
|---|---|---|
| `PermissionDenied` | -32602 | `"Access denied: <orig>. Check the service account's IAM roles for project '<project_id>' (needs at minimum 'roles/datastore.viewer' for reads)."` |
| `ResourceExhausted` | -32603 | `"Firestore quota exceeded: <orig>. Wait a minute and retry. If this persists, check the GCP Quotas page for your project."` |
| `DeadlineExceeded` | -32603 | `"Request timed out: <orig>. The query may be missing an index or scanning a very large collection — try LIMIT to narrow the result set."` |
| `Unavailable` | -32603 | `"Firestore temporarily unavailable: <orig>. This is usually transient — retry in a few seconds."` |

Project ID for the `PermissionDenied` substitution is read from `state::settings()`. If settings aren't initialised (shouldn't happen on this code path but defensive), we fall back to `"the configured project"`.

`map_message` extends to dispatch on the new variants. The classifier extends the substring matcher:

```rust
if raw.contains("FAILED_PRECONDITION")  { ErrorKind::FailedPrecondition }
else if raw.contains("UNAUTHENTICATED") { ErrorKind::Unauthenticated }
else if raw.contains("PERMISSION_DENIED") { ErrorKind::PermissionDenied }
else if raw.contains("NOT_FOUND")       { ErrorKind::NotFound }
else if raw.contains("RESOURCE_EXHAUSTED") { ErrorKind::ResourceExhausted }
else if raw.contains("DEADLINE_EXCEEDED")  { ErrorKind::DeadlineExceeded }
else if raw.contains("UNAVAILABLE")     { ErrorKind::Unavailable }
else                                    { ErrorKind::Other }
```

## Cargo dependencies

The `lru` decision: implement `TtlLruCache` ourselves in `src/cache.rs` rather than pulling in the `lru` crate. The cache is used in two places (cursor cache and count cache) and the implementation is ~80 lines. Dependency-tree-minimalist house style.

No new crates needed for Phase 2 if we go that route. Phase 1's existing deps (serde, tokio, firestore, rustls, once_cell, regex, futures, base64, chrono, gcloud-sdk) cover everything.

## Testing

### Unit tests

- **`query_parser::tests`** — additions:
  - `parses_where_with_eq`, `parses_where_with_double_eq`, `parses_where_with_ne_and_diamond` (`!=` vs `<>`)
  - `parses_string_literal_with_escaped_quote`
  - `parses_int_and_float_literals`, `parses_boolean_and_null_literals`
  - `parses_dot_notation_field_path`
  - `parses_in_and_not_in_lists`
  - `parses_array_contains` and `parses_array_contains_any`
  - `parses_and_chain`, `parses_or_chain`
  - `precedence_or_under_and` (asserts `a OR b AND c` is `Or(a, And(b, c))`)
  - `parens_override_precedence`
  - `rejects_empty_where`, `rejects_unbalanced_parens`, `rejects_empty_in_list`, `rejects_unknown_function`
  - 18–22 new tests, ~30 total in module

- **`firestore_filter::tests`** — pre-flight validation (no Firestore needed, pure logic):
  - `validates_inequality_on_one_field` (passes)
  - `rejects_inequality_on_two_fields`
  - `rejects_in_with_31_values`
  - `rejects_array_contains_with_array_contains_any`
  - `rejects_two_array_contains_on_same_field`
  - 5–7 tests

- **`cache::tests`** — TTL / LRU behaviour:
  - `lookup_miss_on_empty_cache`
  - `inserted_value_is_retrievable`
  - `expired_entry_returns_miss`
  - `lru_eviction_on_capacity`
  - `concurrent_inserts_do_not_corrupt`
  - 5 tests

- **`firestore_error::tests`** — additions:
  - 4 tests for new variants
  - 1 test for project-id substitution in PermissionDenied

- **`schema_infer::tests`** — addition:
  - `reference_value_extracts_target_collection`
  - `mixed_reference_targets_yield_no_fk`

### Integration test

`tests/firestore_emulator.rs` extended with a Phase 2 block (still `#[ignore]`-gated):

1. Seed fixture: 3 collections (`users`, `posts`, `tags`), `posts.author` is a `reference` to `users`, sample docs include map and array fields
2. Walk through:
   - `WHERE email = 'fixture@x.de'` → exactly 1 row
   - `WHERE views > 100 AND status IN ('published', 'draft')` → expected subset
   - `WHERE (priority = 'high' OR priority = 'urgent') AND ARRAY_CONTAINS(tags, 'launch')` → tests OR + ARRAY_CONTAINS together
   - Pagination: `LIMIT 5 OFFSET 0`, then `LIMIT 5 OFFSET 5`, then `LIMIT 5 OFFSET 10` — assert returned doc IDs are disjoint and the second/third calls hit the cursor cache (verified via internal counter or log)
   - `total_count` matches actual seeded row count
   - `get_schema_snapshot` returns the `posts.author → users` foreign key

A `tests/fixtures/seed.sh` script seeds the emulator via `gcloud firestore import` (or the emulator REST API directly).

### Manual smoke tests

Documented in `README.md`'s testing section, run before tagging the Phase 2 release:
- All the queries in the "Accepted query forms" table above against `luninora`
- Sequential pagination over a >1000-row collection, eyeballing the latency of pages 2–10 vs page 1
- A query crafted to hit `PERMISSION_DENIED` (e.g., a collection the SA can't read) — verify the IAM hint message
- A query crafted to hit a missing index (sort by a non-default field on a large collection) — verify Phase 1's index-URL extraction still works
- The native-JSON-for-maps smoke-test gate

## Acceptance criteria

Phase 2 is done when **all of these are green**:

1. `cargo build --release` succeeds with no warnings
2. `cargo clippy --all-targets -- -D warnings` passes
3. `cargo test` passes (expected ~70–80 unit tests; Phase 1 had 40)
4. The integration test in `tests/firestore_emulator.rs` passes against a running Firestore emulator with the Phase 2 fixtures (`cargo test -- --ignored`)
5. Manual end-to-end tests against a real Firestore project (`luninora`) show:
   - All the documented WHERE forms return correct rows
   - Sequential pagination latency is observably constant from page 2 onwards
   - `total_count` displays the real total in the Tabularis grid footer
   - PERMISSION_DENIED, missing-index, and DEADLINE_EXCEEDED errors all surface their hint messages
   - ER-diagram view in Tabularis shows at least one inferred foreign key
6. The native-JSON-for-maps smoke test concludes with a documented decision (kept or reverted)
7. `ROADMAP.md` updated: Phase 2 marked as "shipped <date>", Phase 3 (CRUD) flagged as next
8. CLAUDE.md updated to describe the Phase 2 implemented state

## Open questions

None blocking. Items that need confirmation during implementation, not before:

- Exact firestore-rs 0.48 API names for `FirestoreQueryFilter::Composite::And`/`Or`, `FirestoreQueryFilterCompare::*`, and `FirestoreDb::fluent().count()` — verified at implementation time via rustdoc.
- Exact field names on `firestore::ExplainResults` — verified at implementation time.
- Whether Tabularis renders nested JSON objects in row cells — answered by the smoke-test gate before merge.
- Cache size bounds (currently 100 cursor entries / 200 count entries) — pick at implementation; we can adjust based on observed memory footprint in real use.
