# Phase 1 — Read-only Firestore Driver

**Status:** Draft, ready for implementation planning
**Date:** 2026-05-08
**Scope:** First sub-project of a multi-phase effort to ship a production-ready Tabularis driver plugin for Google Cloud Firestore. Phase 1 delivers a read-only viewer; Phases 2–5 extend it (see "Out of scope" below).

## Goal

A Tabularis user installs the `firestore` plugin, opens Settings, fills in `project_id` (+ optionally `service_account_path`), creates a connection, and can:

1. See the configured Firestore database listed under "Databases".
2. See all root collections of that database listed as "Tables" in the sidebar.
3. Click a collection and see a column header derived from sampled documents, plus the documents themselves rendered as rows in the data grid.
4. Sort columns and paginate via Tabularis' standard grid controls (which dispatch `ORDER BY`, `LIMIT`, `OFFSET` queries).
5. When a query needs a missing composite index, see the Firestore "Create index" URL directly in the error toast.

That's the entirety of Phase 1. No writes, no filters, no subcollections, no UI extensions.

## Non-goals (explicitly Phase 2–5)

- WHERE clauses, JOINs, aggregations, or any non-`SELECT *` query — Phase 2.
- Subcollection navigation — Phase 4 (needs UI extension).
- Per-connection project/database selection (multi-project) — Phase 4 (needs UI extension on `connection-modal.connection_content`).
- INSERT/UPDATE/DELETE row editing — Phase 3.
- DDL — likely permanently `not_implemented` for Firestore.
- ER diagram via `get_schema_snapshot` with reference fields modeled as foreign keys — Phase 2 or 4.
- OAuth token management UI — Phase 4.
- Listener / `watch` integration — Phase 4 or 5.

## User-facing setup flow

1. Install the plugin (download release zip or `just dev-install`).
2. In Tabularis Settings → Plugins → Firestore, fill in:
   - **GCP Project ID** (required)
   - **Database ID** (default `(default)`)
   - **Service Account JSON Path** (optional; if blank, ADC is used)
   - **Firestore Emulator Host** (optional; e.g. `localhost:8080`)
   - **Schema-Inferenz Sample-Größe** (optional; default 50)
3. Open the Connection modal, pick "Firestore" as driver. Because `no_connection_required: true`, only the connection name field is shown. Save.
4. Click the new connection. Tabularis spawns the plugin, sends `initialize` with the saved settings, then `test_connection`. On success, the connection appears connected.
5. Sidebar populates: one database (the configured `database_id`) → root collections under it.

## Architecture

### Manifest changes

Replace the current scaffold `manifest.json` with:

```json
{
  "$schema": "https://tabularis.dev/schemas/plugin-manifest.json",
  "id": "firestore",
  "name": "Firestore",
  "version": "0.1.0",
  "description": "Tabularis driver plugin for Google Firestore",
  "default_port": null,
  "default_username": "",
  "executable": "firestore-plugin",
  "capabilities": {
    "schemas": false,
    "views": false,
    "routines": false,
    "file_based": false,
    "folder_based": false,
    "no_connection_required": true,
    "identifier_quote": "\"",
    "alter_primary_key": false,
    "alter_column": false,
    "create_foreign_keys": false,
    "manage_tables": false,
    "readonly": true
  },
  "settings": [
    { "key": "project_id", "label": "GCP Project ID", "type": "string", "required": true },
    { "key": "database_id", "label": "Database ID", "type": "string", "default": "(default)" },
    { "key": "service_account_path", "label": "Service Account JSON Path", "type": "string",
      "description": "Optional. If empty, falls back to GOOGLE_APPLICATION_CREDENTIALS or gcloud ADC." },
    { "key": "emulator_host", "label": "Firestore Emulator Host", "type": "string",
      "description": "Optional. e.g. localhost:8080. Overrides production endpoint." },
    { "key": "sample_size", "label": "Schema-Inferenz Sample-Größe", "type": "number", "default": 50 }
  ],
  "data_types": [
    { "name": "TEXT",      "category": "string",  "requires_length": false, "requires_precision": false },
    { "name": "INTEGER",   "category": "numeric", "requires_length": false, "requires_precision": false },
    { "name": "REAL",      "category": "numeric", "requires_length": false, "requires_precision": false },
    { "name": "BOOLEAN",   "category": "other",   "requires_length": false, "requires_precision": false },
    { "name": "TIMESTAMP", "category": "date",    "requires_length": false, "requires_precision": false }
  ]
}
```

`readonly: true` and `manage_tables: false` are crucial: Tabularis hides all mutation UI, so the `not_implemented` stubs for CRUD/DDL never get called by the host. `data_types` lists only the most common Firestore-mappable types — they're irrelevant when `manage_tables: false`, but the manifest schema requires the field.

### Module layout

```
src/
├── main.rs               #[tokio::main] dispatch loop (async, BufReader<Stdin>::lines)
├── rpc.rs                async dispatch + ok_response / error_response (extended with optional `data`)
├── error.rs              PluginError (existing)
├── models.rs             ConnectionParams (existing) + Settings (new) + Column / Document shapes
├── client.rs             firestore-rs wiring: build FirestoreDb from Settings (replaces stub)
├── firestore_error.rs    NEW: FirestoreError → JSON-RPC mapping with index-URL extraction
├── schema_infer.rs       NEW: sample-N-docs → Vec<Column> with type-conflict resolution
├── query_parser.rs       NEW: hand-rolled parser for the supported SELECT * grammar
├── handlers/
│   ├── metadata.rs       get_databases, get_tables, get_columns + empty stubs for views/routines/etc
│   ├── query.rs          test_connection, ping, execute_query, explain_query
│   ├── crud.rs           unchanged (all not_implemented)
│   └── ddl.rs            unchanged (all not_implemented)
└── utils/                unchanged (identifiers, pagination)
```

### Runtime

`main.rs` becomes async:

```rust
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        let response = rpc::handle_line(trimmed).await;
        let mut body = serde_json::to_string(&response)
            .unwrap_or_else(|err| format!(
                r#"{{"jsonrpc":"2.0","error":{{"code":-32603,"message":"serialization failed: {err}"}},"id":null}}"#));
        body.push('\n');
        if stdout.write_all(body.as_bytes()).await.is_err() { break; }
        let _ = stdout.flush().await;
    }
}
```

Why `multi_thread`: `firestore-rs` uses `tonic` + `hyper` with internal connection pooling that benefits from a multi-threaded runtime. The dispatch loop itself stays sequential (one request → one response), but background gRPC work parallelises.

Remove the crate-level `#![allow(dead_code)]` from `main.rs` once `Client` is wired up — it's there only because the scaffold leaves `ConnectionParams` and friends unused.

### Global state

```rust
use once_cell::sync::OnceCell;
use parking_lot::RwLock;  // or std::sync::RwLock if we avoid the extra dep

static SETTINGS:     OnceCell<Settings>                      = OnceCell::new();
static CLIENT:       tokio::sync::OnceCell<FirestoreDb>      = tokio::sync::OnceCell::const_new();
static SCHEMA_CACHE: once_cell::sync::Lazy<RwLock<HashMap<String, Vec<Column>>>>
                   = once_cell::sync::Lazy::new(|| RwLock::new(HashMap::new()));
```

- `SETTINGS` filled in `initialize`. If `initialize` is never called (Tabularis ignores init errors silently per the guide, but always calls it), handlers must `return error` if `SETTINGS.get()` is `None`.
- `CLIENT` is `tokio::sync::OnceCell` because its initializer (`FirestoreDb::new()`) is async. Lazy-built on first use (typically `test_connection`), not in `initialize` — the Plugin Guide explicitly states init failures are silently ignored, so we want any auth error to surface from `test_connection` instead.
- `SCHEMA_CACHE` lives for the plugin process lifetime. Cache scope: keyed by collection name. Phase 4 may add manual invalidation.

If `parking_lot` feels like dependency creep, `std::sync::RwLock` is fine here — contention is negligible (one request at a time).

### Settings shape

```rust
#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub project_id: String,
    pub database_id: String,        // default "(default)"
    pub service_account_path: Option<String>,
    pub emulator_host: Option<String>,
    pub sample_size: u32,           // default 50
}

impl Settings {
    pub fn from_value(v: &serde_json::Value) -> Self {
        // tolerant: missing fields → defaults; non-string/number types → defaults
    }
}
```

## RPC handlers

### `initialize`

```rust
"initialize" => {
    let settings = Settings::from_value(&params["settings"]);
    let _ = SETTINGS.set(settings); // ignore error — second initialize after a re-init is a no-op
    ok_response(id, Value::Null)
}
```

### `test_connection`

Lazy-initialise the client, run a cheap probe (`db.list_collections().take(1)` consumed), return `{success: true}` or a mapped error. The probe call validates auth + reachability without scanning the database.

### `ping`

If `CLIENT.get().is_some()` return `null` immediately (Tabularis pings every ~30 s). Otherwise delegate to `test_connection`. The fast path saves a gRPC round-trip per ping.

### `get_databases`

Returns `[settings.database_id]` as a JSON string array (Tabularis spec: array of names, not objects). In Phase 1 this is always a single-element array. Phase 4 with multi-project UI can return all databases the configured credential has access to.

### `get_tables`

Pseudocode (exact firestore-rs call site to be confirmed during implementation — the crate exposes either `client.list_collection_ids()` returning a stream or a similar fluent method):

```rust
let names: Vec<String> = list_root_collections(&client).await?;
let mut tables: Vec<Value> = names.into_iter()
    .map(|n| json!({ "name": n, "schema": null, "comment": null }))
    .collect();
tables.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
ok_response(id, json!(tables))
```

If the Firestore project has hundreds of root collections we accept the latency; in practice it's small. Not cached — collections appear/disappear dynamically.

### `get_columns`

Algorithm:

1. Read `params.table` (the collection name).
2. Hit `SCHEMA_CACHE` — if present, return cached columns.
3. Fetch up to `settings.sample_size` documents via `client.fluent().select().from(table).limit(N).query().await`.
4. Run schema inference (see below) → `Vec<Column>`.
5. Insert into cache, return.

**Schema inference (`schema_infer.rs`):**

- First column is always `__id__` → `{ name: "__id__", data_type: "string", is_nullable: false, column_default: null, is_primary_key: true, is_auto_increment: false, comment: "Firestore document ID" }`.
- For each top-level field across the sample, collect the set of observed Firestore types (`string`, `integer`, `double`, `boolean`, `timestamp`, `bytes`, `geopoint`, `reference`, `array`, `map`, `null`).
- Map to Tabularis `data_type` strings:

  | Firestore type(s) observed | `data_type` |
  |---|---|
  | only `string` | `"string"` |
  | only `integer` or `double`, or both | `"number"` |
  | only `boolean` | `"boolean"` |
  | only `timestamp` | `"timestamp"` |
  | only `bytes` | `"binary"` |
  | only `geopoint` | `"geopoint"` |
  | only `reference` | `"reference"` |
  | only `array` | `"array"` |
  | only `map` | `"map"` |
  | only `null` | `"null"` |
  | any other combination | `"mixed"` |

  `null` co-observed with one other type → that type, plus `is_nullable: true`. (e.g. observed `{string, null}` → `{ data_type: "string", is_nullable: true }`.)

- Field is `is_nullable: true` if at least one sampled doc lacks it OR has it as null.
- `is_primary_key`, `is_auto_increment` always false for non-`__id__`.
- Field order: `__id__` first, then alphabetical.

### `execute_query`

Hand-rolled parser in `query_parser.rs` accepts:

```ebnf
Query   = "SELECT" "*" "FROM" Table OrderBy? Limit? Offset?
Table   = ('"' Ident '"') | ('`' Ident '`') | Ident
OrderBy = "ORDER" "BY" OrderItem ("," OrderItem)*
OrderItem = Ident ("ASC" | "DESC")?
Limit   = "LIMIT" UnsignedInt
Offset  = "OFFSET" UnsignedInt
```

Case-insensitive keywords. Whitespace flexible. Identifiers: `[A-Za-z_][A-Za-z0-9_]*`. Anything else (WHERE, JOIN, aggregate, subquery, non-`*` select list) yields a clear `-32602` error: `"Phase 1 supports only 'SELECT * FROM \"<collection>\" [ORDER BY field [ASC|DESC], ...] [LIMIT n] [OFFSET n]'. WHERE/JOIN/aggregate queries are Phase 2."`.

Mapping to firestore-rs:

```rust
let mut q = client.fluent().select().from(parsed.table.as_str());

let order_items: Vec<(String, FirestoreQueryDirection)> = parsed.order_by.iter()
    .map(|i| (i.field.clone(),
              if i.desc { FirestoreQueryDirection::Descending }
              else      { FirestoreQueryDirection::Ascending }))
    .collect();
if !order_items.is_empty() { q = q.order_by(order_items); }

if let Some(n) = parsed.limit  { q = q.limit(n as u32); }
if let Some(o) = parsed.offset { q = q.offset(o as u32); }

let docs: Vec<FirestoreDocument> = q.query().await?;
```

Build the full ORDER BY list in one pass before calling `.order_by()` once — calling it iteratively would replace previous items rather than accumulate.

`offset()` is documented by Firestore as expensive (server reads + discards skipped docs). Phase 1 accepts this; Phase 2 replaces with cursor-based pagination via `start_after()`.

Response shape:

```rust
{
    "columns": ["__id__", <inferred field names in the same order as get_columns>],
    "rows":    [[<value-per-column>], ...],
    "total_count": rows.len(),
    "execution_time_ms": <measured>
}
```

Value serialisation per Firestore type:

| Firestore type | JSON value in row |
|---|---|
| string | string |
| integer / double | number |
| boolean | boolean |
| timestamp | RFC 3339 string (e.g. `"2026-05-08T14:32:01Z"`) |
| bytes | base64 string |
| geopoint | `{ "lat": …, "lng": … }` JSON object |
| reference | `"projects/.../databases/.../documents/path/doc"` string |
| array | JSON-stringified array |
| map | JSON-stringified object |
| null / missing | `null` |

`total_count` in Phase 1 is always `rows.len()` — no extra COUNT query against Firestore. Phase 2 may add an optional `count()` aggregation.

If the column derived from inference doesn't appear in a particular doc, write `null` in that cell.

### `explain_query`

`not_implemented`. Phase 2 may return Firestore's query plan if the API exposes it.

### Empty / not_implemented stubs

These return empty arrays/objects so Tabularis loads cleanly:
- `get_schemas` → `[]`
- `get_indexes`, `get_foreign_keys` → `[]`
- `get_views`, `get_view_columns`, `get_routines`, `get_routine_parameters` → `[]`
- `get_view_definition`, `get_routine_definition` → `""`
- `get_schema_snapshot` → `{ tables: [], columns: {}, foreign_keys: {} }`
- `get_all_columns_batch`, `get_all_foreign_keys_batch` → `{}`

These return `-32601`:
- `create_view`, `alter_view`, `drop_view`
- `insert_record`, `update_record`, `delete_record`
- All `get_*_sql` and `drop_*` DDL methods

## Error mapping (`firestore_error.rs`)

Centralised mapping from `FirestoreError` (the firestore-rs error type) → `(jsonrpc_code, message, optional data)`. Every handler funnels its error path through this.

```rust
use regex::Regex;
use once_cell::sync::Lazy;

static INDEX_URL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"https://console\.(?:firebase|cloud)\.google\.com/[^\s'\"]+").unwrap()
});

pub fn map_error(err: &firestore::errors::FirestoreError) -> (i64, String, Option<serde_json::Value>) {
    let raw = err.to_string();

    // 1. Missing index — extract URL.
    if is_failed_precondition(err) {
        if let Some(m) = INDEX_URL_RE.find(&raw) {
            let url = m.as_str().to_string();
            return (
                -32603,
                format!("Missing Firestore index. Create it: {url}"),
                Some(serde_json::json!({ "create_index_url": url })),
            );
        }
    }

    // 2. Auth.
    if is_unauthenticated(err) {
        return (
            -32602,
            format!("Auth failed: {raw}. Set service_account_path in plugin settings or run 'gcloud auth application-default login'."),
            None,
        );
    }

    // 3. Not found.
    if is_not_found(err) {
        return (-32602, format!("Not found: {raw}"), None);
    }

    // 4. Fallback.
    (-32603, format!("Firestore: {raw}"), None)
}
```

`is_failed_precondition` / `is_unauthenticated` / `is_not_found` inspect the gRPC status code if firestore-rs exposes it (likely via `FirestoreError::DatabaseError(..).public.code` or similar — to be confirmed during implementation by inspecting the firestore-rs error enum). Fallback: substring-match `"FAILED_PRECONDITION"` / `"UNAUTHENTICATED"` / `"NOT_FOUND"` in the message.

`rpc::error_response` is extended to carry optional `data`:

```rust
pub fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(d) = data { error["data"] = d; }
    json!({ "jsonrpc": "2.0", "error": error, "id": id })
}
```

JSON-RPC 2.0 spec allows optional `data` in error objects (§5.1). Tabularis' error-toast renderer auto-links `https://` URLs; the verbatim URL in the message is what ships in Phase 1. The structured `error.data.create_index_url` is forward-compatible — Phase 4 can wire a "Create Index" button into a UI extension that reads it.

Phase 2 expands this module: `PERMISSION_DENIED` (IAM hint), `RESOURCE_EXHAUSTED` (quota / backoff), `DEADLINE_EXCEEDED` (retry hint), `UNAVAILABLE` (network).

## Cargo dependencies

Add via `cargo add` in this order. Resolved versions as of 2026-05-08:

```bash
cargo add serde --features derive          # serde 1.0.228
cargo add tokio --features rt-multi-thread,macros,io-std,io-util  # tokio 1.52.2
cargo add firestore                         # firestore 0.48.0
cargo add rustls                            # rustls 0.23.40 (TLS provider required by firestore-rs)
cargo add once_cell                         # once_cell 1.21.4
cargo add regex                             # regex 1.12.3
```

`serde_json` is already in `Cargo.toml`. Existing release-profile settings (`lto = true`, `codegen-units = 1`, `strip = "symbols"`) stay.

`firestore` pulls `tonic`, `gcloud-sdk`, `prost` — unavoidable. Review the resulting tree once and decide whether to disable any default features (e.g. `caching-persistent` if we don't use it).

## Testing

### Unit tests

- **`query_parser::tests`** — golden positive set (varied case, whitespace, quoting styles, with/without optional clauses), and a negative set proving `WHERE`, `JOIN`, non-`*` select-lists, missing FROM, etc. all return errors with the expected message.
- **`schema_infer::tests`** — fixed sample documents, expected column lists. Cover: all-string field, mixed string/integer field (→ `mixed`), string+null field (→ `string` + `is_nullable: true`), nested map field (→ `map`), all-null field, missing field in some docs.
- **`firestore_error::tests`** — synthetic error messages with and without index URLs, both `console.firebase.google.com` and `console.cloud.google.com` domains, with `FAILED_PRECONDITION` markers, plus messages that look like URLs but aren't index URLs (false-positive guard).
- Existing tests in `utils/identifiers.rs` and `utils/pagination.rs` stay.

### Integration test (`tests/`)

A single `tests/firestore_emulator.rs` integration test:

- Skipped (`#[ignore]`) unless `FIRESTORE_EMULATOR_HOST` is set, so `cargo test` works without an emulator.
- Spawn the plugin binary as a subprocess.
- Drive it via stdin with a sequence of JSON-RPC lines: `initialize` → `test_connection` → `get_databases` → `get_tables` → `get_columns` → `execute_query`.
- Assert response shapes and content against a seeded fixture in the emulator.
- CI workflow (separate task, may slip to Phase 5) starts the emulator before running `cargo test --ignored`.

### Manual smoke test

`just dev-install`, restart Tabularis, point at a small Firestore project (or the emulator), click around. Document the smoke-test checklist in `README.md` once Phase 1 ships.

The existing `src/bin/test_plugin.rs` REPL stays as-is (echo-only); it's only useful for sanity-checking method names you intend to send.

## Acceptance criteria

Phase 1 is done when:

1. `cargo build --release` succeeds with no warnings.
2. `cargo clippy --all-targets -- -D warnings` passes.
3. `cargo test` passes (including all new unit tests).
4. The integration test passes against a running Firestore emulator.
5. Against a real Firestore project with a service-account JSON, `just dev-install` + restart Tabularis yields:
   - The connection picker offers "Firestore"
   - After saving plugin settings + a connection, clicking Connect succeeds
   - The sidebar shows the configured database with all its root collections
   - Clicking a collection populates the data grid with documents (1+ pages worth)
   - Sorting a column re-issues a query with `ORDER BY` and the grid updates
   - Triggering a missing-index error (sort by a non-default-indexed field on a large collection) shows the Firestore console URL in the error toast, clickable
6. CLAUDE.md is updated with the now-real architecture (replacing the "scaffold + plan" framing).

## Open questions

None blocking. Items that need confirmation during implementation, not before:

- Exact firestore-rs error-introspection API for distinguishing `FAILED_PRECONDITION` / `UNAUTHENTICATED` / `NOT_FOUND` (substring fallback is acceptable if structural access is awkward).
- Whether `data_types` array order matters for Tabularis' UI sort (likely not, since `manage_tables: false`).
- Whether to disable any default firestore-rs features (`caching-persistent`?) once we see the dependency tree.
