# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Tabularis driver plugin targeting **Google Cloud Firestore**. The plugin is a standalone Rust binary (`firestore-plugin`) that Tabularis spawns as a child process and talks to over **JSON-RPC 2.0 on stdio** (one request per line on stdin, one response per line on stdout, stderr free for logging). The same process is reused for the entire connection session.

The current codebase is the unmodified `@tabularis/create-plugin` scaffold renamed to `firestore`. Every method either echoes `{success: true}`, returns an empty array, or returns JSON-RPC `-32601 method not implemented`. The Firestore driver itself is not yet wired up — `client.rs` is an empty `Client { params: ConnectionParams }` waiting for the real implementation.

Naming convention used in this repo:
- Cargo crate / binary: `firestore-plugin` (deliberately suffixed to avoid shadowing the `firestore` crate from crates.io that we'll likely depend on)
- Manifest `id` and plugin install folder: `firestore`
- Manifest `name` (UI label): `Firestore`

## Common commands

All workflows are wrapped in `justfile`:

- `just build` — `cargo build` (also builds `ui/` if a `ui/package.json` exists; no-op otherwise)
- `just release` — release build with LTO + symbol stripping (this is what GitHub Actions ships)
- `just test` — `cargo test` (unit tests live alongside `utils/identifiers.rs` and `utils/pagination.rs`)
- `just lint` — `cargo clippy --all-targets -- -D warnings`
- `just fmt` — `cargo fmt --all`
- `just repl` — runs the `test_plugin` bin for ad-hoc method probing (caveat below)
- `just dev-install` — debug build + copy binary and `manifest.json` into the platform's Tabularis plugin folder (`~/.local/share/tabularis/plugins/firestore` on Linux, `~/Library/Application Support/com.debba.tabularis/plugins/firestore` on macOS, `%APPDATA%\com.debba.tabularis\plugins\firestore` on Windows). Restart Tabularis or toggle the plugin in Settings to pick up changes.
- `just uninstall` — remove the installed plugin folder

Run a single test: `cargo test <test_name>` (e.g. `cargo test escapes_embedded_quotes`).

The toolchain is pinned to stable in `rust-toolchain.toml` with `rustfmt` + `clippy`.

## Tabularis plugin protocol — what the host expects

Source: <https://tabularis.dev/wiki/plugins> and <https://tabularis.dev/wiki/building-plugins>.

### Lifecycle

1. Tabularis spawns the executable named by `manifest.json:executable`.
2. Sends an `initialize` JSON-RPC call carrying user-configured settings (currently a no-op `Null` response in `rpc.rs`).
3. Issues `test_connection` when the user picks the driver from the connection picker.
4. Calls `ping` every ~30 s as a health check; if the plugin doesn't implement `ping`, the host falls back to `test_connection`.
5. On user activity, calls metadata / query / CRUD / DDL methods.

### Method priority for Firestore implementation

The Tabularis docs recommend filling handlers in this order — adapt them to Firestore's collection/document model:

1. `initialize` — receive saved settings (project ID, service-account JSON path, emulator host)
2. `test_connection` — open a `FirestoreDb`, fetch a trivial doc or list root collections, return `{success: true}` or an error
3. `get_databases` / `get_tables` / `get_columns` — see Firestore mapping below
4. `execute_query` — needs a Firestore-flavoured query language (we'll have to design this; SQL doesn't apply natively)
5. `insert_record` / `update_record` / `delete_record` — straightforward via `db.fluent().update()` / `delete()`
6. DDL generators — likely permanently `not_implemented` for Firestore (no schema)

A plugin with only the first three methods is "already useful as a read-only viewer" per the docs, so that's the first MVP target.

### Firestore → Tabularis taxonomy mapping

Tabularis assumes a relational world. For Firestore we'll need to map:
- `get_databases` → either `["(default)"]` plus any named Firestore databases on the project, or just the project ID
- `get_tables` → root collections (returned as `[{ name, schema: null, comment: null }]`)
- `get_columns` → since Firestore is schemaless, sample N documents and union their top-level fields, returning `data_type` from inferred Firestore types (`string`, `number`, `boolean`, `timestamp`, `geopoint`, `reference`, `array`, `map`, `null`)
- `execute_query` → wrap Firestore filters/orderBy/limit; consider supporting a pseudo-SQL subset or a JSON query DSL
- Subcollections — there is no relational analogue; consider exposing them via `get_schema_snapshot` or a UI extension slot

Capability flags in `manifest.json` need to reflect this:
- `schemas: false`, `views: false`, `routines: false` — none apply
- `manage_tables: false`, `alter_column: false`, `alter_primary_key: false`, `create_foreign_keys: false`
- `readonly: true` until CRUD is wired up; flip to `false` afterwards
- `identifier_quote` — irrelevant to Firestore; leave at `"` so the host has *something*
- The default port `5432` is meaningless for Firestore — set to `null` (and consider adding a custom settings UI for project ID + creds via a `ui_extensions` slot) once we move past the scaffold

### Required response shapes (host contract)

| Method | Shape |
|---|---|
| `test_connection` | `{ "success": true }` or error |
| `get_databases` | `["name", ...]` |
| `get_tables` | `[{ name, schema, comment }]` |
| `get_columns` | `[{ name, data_type, is_nullable, column_default, is_primary_key, is_auto_increment, comment }]` |
| `execute_query` | `{ columns: string[], rows: any[][], total_count: number, execution_time_ms: number }` |
| `get_schema_snapshot` | `{ tables, columns: { [t]: cols }, foreign_keys: { [t]: fks } }` (ER-diagram batch) |

### Error code conventions

`-32700` parse error · `-32600` invalid request · `-32601` method not found · `-32602` invalid params · `-32603` internal. The `error::PluginError` type carries `(code, message)` and is intentionally hand-rolled — no `anyhow`/`thiserror` to keep the dependency tree small.

## Architecture

### Stdio dispatch loop

`main.rs` is a tight `BufRead::lines()` loop: read one JSON line, hand it to `rpc::handle_line`, write the serialised response + newline to stdout, flush. No threading, no async. If serialisation itself fails, a hand-rolled `-32603` error string is written so the host always sees valid JSON-RPC. This will need to switch to **async** once `firestore-rs` (Tokio-based) is wired in — see "Firestore-rs integration" below.

`rpc::handle_line` is the single match table mapping method name → handler. Three response helpers in `rpc.rs`: `ok_response`, `error_response`, `not_implemented`.

### Handler layout

`handlers/` is split by concern, mirroring the Tabularis plugin contract:
- `metadata.rs` — `get_databases/schemas/tables/columns/indexes/foreign_keys`, view + routine introspection, plus `get_schema_snapshot` (the ER-diagram batch endpoint) and `get_all_*_batch`
- `query.rs` — `test_connection`, `execute_query`, `explain_query` (`ping` is handled directly in `rpc.rs` and returns `Null`)
- `crud.rs` — row-level `insert/update/delete_record`
- `ddl.rs` — `get_*_sql` generators + `drop_index/drop_foreign_key`

Stubs return either empty arrays (so the sidebar loads without errors) or `-32601`. Pick stubs to flesh out based on which Tabularis UI surfaces you want to enable.

### Connection params shape

Tabularis wraps connection params one level deeper than you might expect: the inner object lives at `params.params` in the request. Use `models::inner_params(&params)` before calling `ConnectionParams::from_value` — every connection-aware handler will need this. For Firestore we'll likely repurpose `host` as the project ID and `password` as either the service-account JSON path or a token, or extend `ConnectionParams` with Firestore-specific fields.

### Driver layer (the empty seat) — Firestore-rs integration

`client.rs` is a deliberately near-empty `Client { params: ConnectionParams }`. The plan is to wire <https://github.com/abdolence/firestore-rs> (`firestore` crate, currently 0.48):

```toml
firestore = "0.48"
rustls = "0.23"   # required TLS provider
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

Auth resolution order (per firestore-rs):
1. `GOOGLE_APPLICATION_CREDENTIALS` env var → service-account JSON
2. `gcloud auth application-default login` (note: ADC, **not** `gcloud auth login`)
3. GCE metadata server when running on Google infrastructure
4. Explicit: `FirestoreDb::with_options_service_account_key_file()`

Emulator support: set `FIRESTORE_EMULATOR_HOST=localhost:8080` before launching Tabularis. We should expose this as a setting.

Common gotchas the docs flag:
- "Crypto provider error" → install `rustls`
- Transactions can't auto-generate doc IDs — always supply explicit IDs in `update()`
- Docker images need root CA certs for TLS

Once `Client::connect` is wired, **remove the crate-level `#![allow(dead_code)]` in `main.rs`** so the compiler starts flagging genuinely unused code again. The dispatch loop will need a Tokio runtime — either `#[tokio::main]` on `main` or block_on inside each handler.

### REPL caveat

`src/bin/test_plugin.rs` does **not** share modules with `main.rs` and does **not** invoke the real dispatcher — it echoes the request back inside a fake `result`. It's only useful for sanity-checking method names you intend to send. For end-to-end testing, run the main binary and pipe JSON-RPC lines into it directly, e.g.:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"test_connection","params":{"params":{}}}' | cargo run
```

### Release pipeline

Tag a commit `v*` and push: `.github/workflows/release.yml` builds for Linux x64/arm64, macOS x64/arm64, and Windows x64, then attaches zipped plugin bundles to the GitHub Release. To publish to the Tabularis in-app registry, open a PR against `plugins/registry.json` in the Tabularis repo.

## Workflow

- **Plan first** — enter plan mode for any non-trivial task (3+ steps or architectural decisions). Create a GitHub issue with the plan (see below). Check in before starting
- **Todos = GitHub issues** — every todo lives as an issue on `Fitdrop/FitDrop`. `tasks/todo.md` is only an index of links. When the user says "new todo", create a GitHub issue with the right labels and add a one-line ref to `tasks/todo.md`. Designs go in the issue body, not in separate files. No `tasks/design/` folder
- **Labels** — area: `chat`, `security`, `platform`, `wardrobe`. Type: `bug`, `enhancement`. Status: `maybe`, `on-hold`. Record-only: `history` (for closed issues kept as historical record)
- **Closing todos** — when finishing work, close the issue via the PR/commit (`Closes #N`) or `gh issue close`. Keep completed work in closed-issue history, not in `tasks/todo.md`
- **Re-plan on failure** — if something goes sideways, stop and re-plan immediately
- **Use subagents** — offload research, exploration, and parallel analysis. One task per subagent, keep main context clean
- **Verify before done** — never mark complete without proving it works. Run tests, check logs, demonstrate correctness. "Would a staff engineer approve this?"
- **Demand elegance** — for non-trivial changes, pause and ask "is there a more elegant way?" Skip for simple fixes
- **Autonomous bug fixing** — given a bug report, just fix it. Zero context switching for the user
- **Learn from mistakes** — after any correction, update `tasks/lessons.md` with the pattern. Review at session start
