# Todo / Followups

Plain index of stuff we want to come back to. Each entry is one line; if something needs design, link to a spec under `docs/superpowers/specs/`.

## Upstream (Tabularis core)

- [ ] **File Tabularis issue: NewRowModal silently drops empty required fields** — `src/components/modals/NewRowModal.tsx:195` skips fields where `rawVal === "" && !col.is_nullable` instead of blocking submit. Relational drivers happen to fail later via NOT NULL constraint, but schemaless drivers (Firestore) just accept the partial doc. Fix: HTML5 `required` attribute + client-side pre-submit check. Plugin workaround already in `handlers/crud.rs::find_missing_required_fields`, but server-roundtrip-validated UX is worse than client-side. Repo: `TabularisDB/tabularis`.
- [ ] **File Tabularis issue: `driver_specific` HashMap on ConnectionParams** — `src-tauri/src/models.rs:110` only carries relational-DB fields. Plugin drivers with per-connection identity (project_id, database_id, service_account_path, schema_overrides_dir) have no place to put per-connection config — everything lives in global plugin settings. Adding `driver_specific: HashMap<String, JsonValue>` (persisted + sent through to plugin RPC calls) enables a `connection-modal.connection_content` UI extension to contribute per-connection fields. Phase-4 enabler. Repo: `TabularisDB/tabularis`.
- [ ] **File Tabularis issue: plugin-manifest `icon` field** — `plugins/manifest.schema.json` has `additionalProperties: false`, so external drivers can't ship custom icons via manifest. Repo: `TabularisDB/tabularis`.
- [ ] **Watch Tabularis #24 — JSON/JSONB Editor & Viewer** (https://github.com/TabularisDB/tabularis/issues/24). When it ships, revert commit `63e0912` (Map/Array stringification) and re-enable native JSON; expose `JSON` in `manifest.json:data_types` and have `schema_infer` emit `data_type: "json"` for Map/Array. Fixes the `[object Object]` hover symptom.

## firestore-driver (this repo)

### Parser

- [ ] Replace hand-rolled `src/query_parser.rs` with [`sqlparser`](https://crates.io/crates/sqlparser) (Apache DataFusion). Reason: Tabularis sends real SQL with subqueries / aliases (`SELECT * FROM (SELECT ... LIMIT n) AS limited_subset`); we patch each quirk by hand today. Migration: parse via `sqlparser::Parser::parse_sql(&GenericDialect{}, sql)`, walk `Query`/`SetExpr::Select`, map to existing `ParsedQuery`. Keep `FilterExpr`/`OrderItem`. Trigger when the next Tabularis-side SQL surprise lands, OR when DML SQL in the Console tab becomes worth it.
- [ ] DML SQL in Console tab (`INSERT INTO ... SET ...`, `UPDATE ... SET ...`, `DELETE FROM ...`). Today returns a friendly redirect via parser intercept. Falls naturally out of the `sqlparser` migration above — wire DML AST → existing CRUD handlers.

### Test coverage

- [ ] Property-based parser tests via `proptest`. The parser surface is wide (precedence, escape sequences, lexer edge cases). PB tests would catch corners hand-written tests miss. Maybe obviated by the `sqlparser` migration.
- [ ] `handlers/metadata.rs` is still ~0% unit-covered (large async I/O surface). Two options: (a) introduce `trait FirestoreOps` + `mockall` for fine-grained unit tests, or (b) lean on the integration suite (already covers the happy paths). Re-evaluate after a few months — if metadata bugs slip through, do (a).
- [ ] CI workflow that runs `just test-integration` on every PR. Needs Java 21 + bun on the runner. GitHub Actions has both via setup-java and oven-sh/setup-bun.

### Phase 4 candidates (parked until concrete need)

- [ ] Multi-database per Firestore project — `get_databases` would call Firestore Admin API; per-call `database` parameter routes to the right `FirestoreDb`. State becomes `RwLock<HashMap<String, FirestoreDb>>`. Depends on the upstream `driver_specific` change for clean per-connection routing.
- [ ] Subcollections — see ROADMAP Phase 4 for the three options (hierarchical sidebar / pseudo-table naming / schema-snapshot only).
- [ ] Auth-UX UI extension — file picker for service-account JSON, ADC indicator, "Use Emulator" toggle. Requires the `connection_content` slot.
- [ ] Real-time listener "Live mode" toggle on the data grid. Requires server-initiated JSON-RPC notifications — verify Tabularis' bridge supports them first.

### Stretch

- [ ] Schema-cache TTL + manual "Refresh schema" action. Today the cache lives for the plugin process; Tabularis restart needed after external writes change the field shape.
- [ ] Settings reload without restart — `SETTINGS` from `OnceCell` to `RwLock<Settings>` so re-init reflects edited plugin settings. Helps the multi-project use case until the upstream `driver_specific` lands.
- [ ] Composite-index suggestions — when a query needs a missing composite index, surface the field paths + directions (so user can paste into `firestore.indexes.json` for IaC) on top of the console URL.
- [ ] Export collection to NDJSON / BigQuery action.
