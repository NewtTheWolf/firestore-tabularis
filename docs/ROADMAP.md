# Roadmap

This file tracks the planned evolution of the firestore Tabularis plugin. Phase 1 shipped on 2026-05-08; phases 2–5 are sketched here so future-Claude / future-user can pick up the next slice without re-deriving context. Each phase becomes a standalone spec → plan → implementation cycle when started.

## Status snapshot

| Phase | Status | Spec |
|---|---|---|
| 1 — Read-only MVP | ✅ shipped 2026-05-08 | [`specs/2026-05-08-phase-1-firestore-read-only-driver-design.md`](specs/2026-05-08-phase-1-firestore-read-only-driver-design.md) |
| 2 — Query layer + Map polish | ✅ shipped 2026-05-08 | [`specs/2026-05-08-phase-2-firestore-query-layer-design.md`](specs/2026-05-08-phase-2-firestore-query-layer-design.md) |
| 3 — CRUD | ✅ shipped 2026-05-09 | (no spec doc — implemented from review-driven plan, see `tasks/todo.md` history) |
| 4 — Multi-DB + Subcollections + Auth UX | not started | TBD |
| 5 — Release & distribution | not started | TBD |

---

## Phase 2 — Query layer + Map polish ✅ shipped 2026-05-08

Shipped: full WHERE/AND/OR/NOT IN/parens grammar, six comparison ops, IN, ARRAY_CONTAINS / ARRAY_CONTAINS_ANY, TIMESTAMP literals, parallel COUNT aggregation with TTL cache for `total_count`, cursor-based pagination via CURSOR_CACHE with OFFSET fallback, ER-diagram via inferred reference foreign keys, four new structured error mappings (PERMISSION_DENIED / RESOURCE_EXHAUSTED / DEADLINE_EXCEEDED / UNAVAILABLE), and `explain_query` returning the Firestore query plan. Native-JSON-for-maps decision: reverted — Tabularis hover-tooltip rendered objects as `[object Object]`, so Map/Array cells stay JSON-stringified for now (Tabularis-side bug to file before re-enabling native JSON in Phase 4's data-grid UI extension).

The biggest UX hebel after Phase 1. Turns the plugin from "data browser" into "daily driver".

**Query expansion**
- `WHERE field <op> value` mapped to Firestore `.filter()`. Operators: `==`, `!=`, `<`, `>`, `<=`, `>=`, `IN (...)`, `NOT IN (...)`, plus a Firestore-specific `array_contains` (proposed syntax: `WHERE tags ARRAY_CONTAINS 'x'`).
- AND-only conjunction (Firestore composite-filter restriction). OR support requires Firestore `Filter.or()` and probably costs an explicit grammar choice.
- Cursor-based pagination via `start_after()` replaces OFFSET — fixes the OFFSET cost-blowup beyond a few thousand rows.
- `COUNT(*)` via the Firestore aggregation API for real `total_count` (Phase 1 returns `rows.len()`).
- Query parser: rewrite or extend hand-rolled parser to support the WHERE/IN grammar; consider whether a small parser-combinator crate is worth pulling in.
- `explain_query` returns the Firestore query plan (the `explain()` API).

**Map / array polish**
- Switch from JSON-stringified strings (Phase 1) to **native JSON objects/arrays in the row payload**, so Tabularis can render them with whatever nested-value handling it has. Probe what Tabularis actually does with non-string row values before committing — fall back to stringified if it crashes the renderer.
- Optional dot-notation flatten mode (`address.city`, `address.zip`) gated by a plugin-setting `flatten_maps` (default off) — opt-in because it's lossy when fields are sparse.

**Error mapping expansion**
- `PERMISSION_DENIED` → IAM-hint message ("check that the service account has roles/datastore.viewer or roles/datastore.user")
- `RESOURCE_EXHAUSTED` → quota-backoff hint
- `DEADLINE_EXCEEDED` → retry hint with current backoff state
- `UNAVAILABLE` → network/transient hint

**ER diagram support (optional sub-feature)**
- `get_schema_snapshot` populated with `reference` fields modeled as foreign keys → ER-diagram in Tabularis shows the relationships between collections.

---

## Phase 3 — CRUD ✅ shipped 2026-05-09

Shipped: `insert_record` (explicit-id or server-side autogen), `update_record` (single-field via `update_only`, rejects `colName=='id'` because Firestore has no in-place doc rename), `delete_record`. New `coercion.rs` module is the inverse of `query::serialize_value`: edit-cell JSON → Firestore proto value, hinted by the inferred schema's `data_type` (timestamps parse RFC3339, references emit ReferenceValue, array/map strings JSON-parse because Phase 2 ships them stringified). All three handlers invalidate COUNT_CACHE + CURSOR_CACHE for the touched table. Manifest flipped to `readonly: false`.

Decisions taken:
- **Optimistic concurrency** — not honored. Tabularis doesn't surface `update_time`; last writer wins. Revisit if multi-user scenarios bite.
- **DDL** — stays `not_implemented`. Firestore creates collections implicitly on first doc write; mapping "Create Collection" to anything meaningful would be theatre. Documented in handlers/ddl.rs.

---

## Phase 4 — Multi-DB + Subcollections + Auth UX

The phase that makes the plugin feel "production-grade" rather than "single-project demo". UI extensions live here, so the React + Vite build pipeline (`ui/` directory) is set up in this phase.

### Multi-database per Firestore project

A Firestore project can host multiple named databases beyond `(default)` (created via `gcloud firestore databases create`). Phase 1 hardcodes a single `database_id` in plugin settings. Phase 4 lifts this:

- `get_databases` makes a real Firestore Admin API call (`projects/<project>/databases` listing) and returns **all databases the credential has access to**, not just the configured one.
- Each RPC handler reads the database identifier from `params.params.database` (Tabularis sends the user's database selection there) and falls back to `settings.database_id` only if the field is empty.
- `state::CLIENT` switches from a single `OnceCell<FirestoreDb>` to a `RwLock<HashMap<String, FirestoreDb>>` keyed by database id — each unique database gets its own lazily-built client, cached for the plugin lifetime.
- `SCHEMA_CACHE` key changes from `<collection>` to `<database>::<collection>` so two databases with same-named collections don't conflict.
- Tabularis UI behaviour: the existing database picker in the sidebar (already there for SQL drivers) lets the user switch between (default), `analytics`, `staging`, etc. without recreating the connection.

### Multi-project per connection (related but distinct)

Currently project_id is plugin-wide. With a UI extension on `connection-modal.connection_content` (driver-filtered) the user can override project_id per connection — so one Tabularis install can browse `prod-firestore`, `staging-firestore`, `dev-firestore` simultaneously without swapping plugin settings.

- React + Vite UI extension that contributes form fields (project_id, database_id, optional service_account_path) to the connection modal
- Custom `ConnectionParams.driver_specific` field carries these per-connection overrides
- Plugin reads them from `params.params` and overrides plugin-wide settings
- Phase 1's plugin-wide settings stay as the **default fallback** for users who don't customise per connection

### Subcollections

The hardest mapping of Firestore → relational. A document under a root collection can host its own subcollections (e.g. `users/abc123/orders/xyz`), arbitrarily nested. Phase 4 options:

1. **Hierarchical sidebar**: Tabularis' `manage_tables: false` already gives us the freedom to render a custom tree. Each table-node is expandable to show its subcollections (one level at a time, lazily fetched via `db.fluent().list().collections().parent("users/abc123").stream_all()`). New manifest-level UI extension or Tabularis-side feature request.
2. **Pseudo-table naming**: expose subcollections as virtual tables with names like `users/{docId}/orders` and the user selects a parent doc via a connection-modal picker. Less elegant but doesn't need new Tabularis UI surface.
3. **Schema-snapshot only**: expose subcollections in the ER diagram but not as queryable tables in the sidebar. Read-only convenience without the UX complexity. Smallest scope.

Decision deferred to Phase 4 brainstorming — depends on what UI surface Tabularis exposes by then.

### Auth UX

UI extension on `settings.plugin.before_settings` slot:
- "Pick Service Account JSON" button → file picker → fills `service_account_path`
- ADC-status indicator: shows whether `gcloud auth application-default login` has been run, with a "Login with gcloud" button that opens a terminal
- "Use Emulator" toggle → reveals an inline emulator-host field
- "Test connection now" button that calls `test_connection` and shows the result in the panel (rather than the user having to create a connection first)

### Maps / arrays — interactive grid rendering

Building on Phase 2's native JSON values: a UI extension on `data-grid.context-menu.items` adds a "View as JSON" menu item that opens a modal with the formatted nested structure. Right-click a `map`-typed cell → expandable JSON tree.

### Listener / watch (optional sub-feature)

Real-time updates: a UI extension on the data grid toolbar adds a "Live mode" toggle that opens a Firestore listener on the current query and streams updates into the grid. Requires us to send unsolicited `notification` RPC frames (rather than only responses to host requests) — confirm that Tabularis' JSON-RPC client supports server-initiated frames before committing.

---

## Phase 5 — Release & distribution

- Full integration-test matrix in CI (start emulator container, seed fixtures, run `cargo test -- --ignored` on Linux + macOS + Windows)
- Documentation pass: dedicated README sections for auth setup, query syntax, multi-DB, troubleshooting common errors (missing-index URL, permission denied, etc.)
- GitHub Release tag `v1.0.0` → existing `release.yml` workflow builds platform bundles, attaches zips
- PR against `TabularisDB/tabularis/plugins/registry.json` adding our entry → plugin appears in the in-app plugin store
- Post-release: monitor issues for a few weeks, cut a `v1.0.1` for whatever feedback comes in

---

## Stretch goals (no fixed phase)

- **Schema-cache TTL + manual "Refresh schema" action**: today the cache lives for the plugin process; a stale schema after collection writes from outside the plugin requires a Tabularis restart.
- **Settings refresh without restart**: today `SETTINGS` is `OnceCell`, second `initialize` is a no-op. Switch to `RwLock<Settings>` so re-init reflects edited plugin settings.
- **Composite-index suggestions**: when a query needs a missing composite index, in addition to the console URL, surface the suggested index *definition* (field paths + directions) so the user can paste it into a `firestore.indexes.json` for IaC.
- **Firestore Security Rules viewer**: read-only display of the current rules (via Firebase Admin API).
- **Export collection to BigQuery / NDJSON**: a "Export" action on a table → either dumps to NDJSON locally or kicks off a BigQuery export job.
- **Aggregation queries**: SUM, AVG, COUNT(field IS NOT NULL) — Firestore has limited aggregation support; expose what's there.
- **Saved queries / collections-of-interest**: bookmark frequent queries per connection. Probably a Tabularis-side feature though, not a plugin concern.
