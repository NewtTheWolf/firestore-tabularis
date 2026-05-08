# Firestore — Tabularis Plugin

Driver plugin for [Tabularis](https://github.com/TabularisDB/tabularis) that connects to [Google Cloud Firestore](https://cloud.google.com/firestore).
Generated with `@tabularis/create-plugin`.

## Getting started

```bash
just dev-install       # build + install into ~/.local/share/tabularis/plugins/firestore
```

Then open Tabularis — your driver appears in the connection picker.

## What's implemented (Phase 1)

| Method | Status | Notes |
|--------|--------|-------|
| `initialize` | implemented | reads plugin-wide settings (`project_id`, `database_id`, `service_account_path`, `emulator_host`, `sample_size`) |
| `test_connection`, `ping` | implemented | builds `FirestoreDb` lazily; probes by listing root collections; ping fast-path returns `null` when client is already up |
| `get_databases` | implemented | returns `[database_id]` from settings |
| `get_tables` | implemented | lists root Firestore collections, alphabetical |
| `get_columns` | implemented | samples up to `sample_size` documents per collection, infers types, caches per-process |
| `execute_query` | partial | accepts `SELECT * FROM "<col>" [ORDER BY field [ASC\|DESC], …] [LIMIT n] [OFFSET n]`. Anything else (WHERE/JOIN/aggregations) returns a clear "Phase 2" error |
| `explain_query`, `insert_record`, `update_record`, `delete_record` | `-32601` | Phase 3 |
| DDL generators (`get_*_sql`, `drop_*`) | `-32601` | likely permanently not_implemented for Firestore |
| `create_view`, `alter_view`, `drop_view` | `-32601` | Firestore has no views |
| `get_views*`, `get_routines*`, `get_indexes`, `get_foreign_keys` | empty arrays | Firestore has no relational analogues |

## Layout

```
src/
├── main.rs              thin stdio loop
├── rpc.rs               method dispatch + response helpers
├── error.rs             plugin error type
├── models.rs            ConnectionParams + common shapes
├── client.rs            connection config — builds FirestoreDb from settings
├── state.rs             globals: SETTINGS, CLIENT, SCHEMA_CACHE
├── firestore_error.rs   error mapping with missing-index URL extraction
├── schema_infer.rs      sample-based column inference
├── query_parser.rs      SELECT * SQL parser
├── handlers/
│   ├── metadata.rs      databases, schemas, tables, columns, indexes, FKs, views, routines
│   ├── query.rs         test_connection, ping, execute_query, explain_query
│   ├── crud.rs          insert_record, update_record, delete_record
│   └── ddl.rs           CREATE/ALTER/DROP generators
├── utils/
│   ├── identifiers.rs   quote_identifier(name) + tests
│   └── pagination.rs    paginate(query, page, size) + tests
└── bin/
    └── test_plugin.rs   local REPL for simulating Tabularis calls
```

## Testing without Tabularis

```bash
just repl
# > get_tables
# { "tables": [] }
```

## Publishing

Tag a commit `v0.1.0` and push — the included GitHub Actions workflow builds for Linux (x64/arm64), macOS (x64/arm64), and Windows (x64), then attaches the zipped plugin bundles to the release. Submit a PR to `plugins/registry.json` in the Tabularis repo to publish to the in-app registry.

## References

- [Plugin guide](https://github.com/TabularisDB/tabularis/blob/main/plugins/PLUGIN_GUIDE.md)
- [Manifest schema](https://github.com/TabularisDB/tabularis/blob/main/plugins/manifest.schema.json)
- [Tabularis repo](https://github.com/TabularisDB/tabularis)

## License

Apache-2.0
