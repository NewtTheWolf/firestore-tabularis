# Firestore — Tabularis Plugin

Driver plugin for [Tabularis](https://tabularis.dev) that connects to
[Google Cloud Firestore](https://cloud.google.com/firestore).

The plugin is a standalone Rust binary that Tabularis spawns as a child
process and talks to over JSON-RPC 2.0 on stdio. It maps Firestore's
collection/document model onto Tabularis' relational worldview so you get
table browsing, schema inference, SQL-flavoured queries, and CRUD against
any Firestore project.

> **Status:** Phases 1–3 shipped (read-only browser, query layer, CRUD).
> Phase 4 (multi-DB / subcollections / live mode) is on the roadmap.

## Install

```bash
git clone ssh://git@codeberg.org/NewtTheWolf/firestore-tabularis.git
cd firestore-tabularis
just plugin install   # debug build + copy into the Tabularis plugin folder
```

Restart Tabularis (or toggle the plugin in Settings) and pick **Firestore**
from the connection picker.

Plugin folders by platform:

| OS | Path |
|---|---|
| Linux | `~/.local/share/tabularis/plugins/firestore` |
| macOS | `~/Library/Application Support/com.debba.tabularis/plugins/firestore` |
| Windows | `%APPDATA%\com.debba.tabularis\plugins\firestore` |

## Authentication

The plugin uses the standard Google authentication chain. Pick whichever
fits your setup:

1. **Service account JSON** — set the path in plugin settings.
2. **Application Default Credentials** — `gcloud auth application-default login`.
3. **`GOOGLE_APPLICATION_CREDENTIALS` env var** — path to a JSON key.
4. **Local emulator** — set the *Firestore Emulator Host* setting (e.g.
   `localhost:8080`) and skip credentials entirely.

Required IAM role for production: `roles/datastore.viewer` (read-only) or
`roles/datastore.user` (read + write).

## What's implemented

| Method | Status | Notes |
|---|---|---|
| `initialize`, `test_connection`, `ping` | ✅ | Connection lifecycle. |
| `get_databases` | ✅ | Returns the configured database id. |
| `get_tables` | ✅ | Lists root collections, alphabetical. |
| `get_columns` | ✅ | Samples N docs, infers types, caches per-process. Honours schema-overrides if configured. |
| `get_schema_snapshot` | ✅ | ER-diagram batch endpoint with inferred reference foreign keys. |
| `execute_query` | ✅ | `SELECT` with WHERE / AND / OR / NOT / IN / NOT IN / ARRAY_CONTAINS / ARRAY_CONTAINS_ANY / six comparison ops / parens / TIMESTAMP literals / ORDER BY / LIMIT / OFFSET / cursor pagination. |
| `explain_query` | ✅ | Returns Firestore EXPLAIN/ANALYZE plan with `documents_returned`, `documents_scanned`, `index_used`, `execution_duration_ms`. |
| `insert_record` / `update_record` / `delete_record` | ✅ | Including doc-id rename via id-cell-edit (read → create → delete). |
| Required-field validation | ✅ | Pre-insert check via [schema overrides](docs/schema-overrides.md). |
| DDL generators (`get_*_sql`, `drop_*`) | ⛔ | Not applicable — Firestore is schemaless. |
| `get_views*`, `get_routines*`, `get_indexes`, `get_foreign_keys` | ✅ (empty) | Firestore has no relational analogues. |

DML over SQL (`INSERT INTO …`, `UPDATE … SET …`, `DELETE FROM …`) currently
returns a friendly redirect explaining to use Tabularis' grid actions
instead. Real DML SQL is on the roadmap together with the `sqlparser`
migration — see [`tasks/todo.md`](tasks/todo.md).

## Settings

Configured per plugin install in **Plugin Settings → Firestore**:

| Key | Required | Description |
|---|---|---|
| `project_id` | ✅ | GCP project that hosts the Firestore database. |
| `database_id` | | Defaults to `(default)`. Set if you use named Firestore databases. |
| `service_account_path` | | Optional. Falls back to ADC / `GOOGLE_APPLICATION_CREDENTIALS`. |
| `emulator_host` | | Optional. e.g. `localhost:8080`. Routes traffic to the emulator. |
| `sample_size` | | Number of documents sampled per collection for type inference (default 50). |
| `schema_overrides_dir` | | Optional. Directory holding per-(project, db) override JSON files. See [`docs/schema-overrides.md`](docs/schema-overrides.md). |

## Layout

```
src/
├── main.rs              thin async stdio loop
├── rpc.rs               method dispatch + response helpers
├── error.rs             plugin error type
├── models.rs            ConnectionParams + common shapes
├── client.rs            connection config — builds FirestoreDb from settings
├── state.rs             globals: SETTINGS, CLIENT, SCHEMA_CACHE, CURSOR_CACHE, COUNT_CACHE, SCHEMA_OVERRIDES
├── cache.rs             generic TTL+LRU cache
├── coercion.rs          JSON → Firestore proto value (insert/update side)
├── firestore_error.rs   gRPC status mapping with missing-index URL extraction
├── firestore_filter.rs  WHERE-tree → firestore-rs filter
├── query_parser.rs      hand-rolled SELECT parser (sqlparser migration on roadmap)
├── schema_infer.rs      sample-based column inference
├── schema_overrides.rs  per-(project, db) JSON override files
├── handlers/
│   ├── metadata.rs      databases, tables, columns, schema_snapshot, etc.
│   ├── query.rs         test_connection, execute_query, explain_query
│   ├── crud.rs          insert/update/delete + doc-id rename
│   └── ddl.rs           CREATE/ALTER/DROP generators (mostly not_implemented)
└── utils/
    ├── identifiers.rs
    └── pagination.rs
tests/
├── firestore_emulator.rs   integration suite (gated #[ignore], run via `just emulator test`)
└── fixtures/               bun-native fixture seeder + free-port helper
```

## Development

Workflows live in a root `justfile` plus three modules under `just/`. Run
`just --list` for the root, `just --list <module>` for a module.

```bash
just build              # cargo build (and ui/ if present)
just release            # release build with LTO + symbol stripping
just test               # cargo test (unit tests only — integration tests are gated)
just cov                # region-level coverage via cargo-llvm-cov
just lint               # cargo clippy --all-targets -- -D warnings
just fmt                # cargo fmt --all
just plugin install     # debug build + install into Tabularis plugin folder
just emulator test      # self-contained integration suite (random port, seed, run, clean up)
```

The integration suite needs **bun** + **Java 21+** (firebase-tools v15
requirement). See [`docs/integration-tests.md`](docs/integration-tests.md)
for the full setup.

## Documentation

- [`docs/ROADMAP.md`](docs/ROADMAP.md) — phase status + what's planned next.
- [`docs/schema-overrides.md`](docs/schema-overrides.md) — power-user
  required-fields / type corrections / hidden columns.
- [`docs/integration-tests.md`](docs/integration-tests.md) — emulator setup.
- [`tasks/todo.md`](tasks/todo.md) — current followups + parked items.

## Contributing

Issues and patches welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md). The
roadmap and todo file flag good first contributions.

## License

Apache-2.0 — see [`LICENSE`](LICENSE).
