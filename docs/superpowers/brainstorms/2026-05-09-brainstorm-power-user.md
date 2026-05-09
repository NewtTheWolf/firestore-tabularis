# Brainstorm: Power-User / Daily-Driver

**Date:** 2026-05-09
**Lens:** Real pain points the user hits while using Tabularis on `luninora-dev` daily. Pragmatic, no "maybe-future-platform" speculation. Ordered by frequency of pain.

---

## Daily friction (encountered ≥ once per session today)

### P1 — Switching dev ↔ prod requires settings-edit + Tabularis restart

**Pain:** Today the project_id is plugin-global. To go from `luninora-dev` to `luninora` (prod) the user opens plugin settings, edits, then has to restart Tabularis because the plugin process re-reads settings only on spawn.

**Want:** Two parallel connections (`luninora-dev` + `luninora`), each with its own Firestore project, switchable via the sidebar without restart.

**Path:**
- Short-term workaround: **flip `Settings` from `OnceCell` to `RwLock<Settings>` so re-`initialize` updates** (~1 h). Doesn't add multi-connection but lets settings reload when Tabularis re-inits the plugin.
- Long-term: Tabularis upstream `driver_specific` HashMap on ConnectionParams, then a `connection-modal.connection_content` UI extension. Two-side change.

**Effort:** Short-term 🟢 1 h · Long-term 🔴 multi-day across two repos.

### P2 — `[object Object]` in cell hover for map/array fields

**Pain:** Right-click → hover → "[object Object]" instead of the JSON content. Already filed as Tabularis #24, but in the meantime every map/array hover is useless.

**Want:** Either a tooltip with formatted JSON, or a "View as JSON tree" right-click action.

**Path:**
- UI-Extension on `data-grid.context-menu.items` ("View as JSON tree" action that opens a modal with the parsed structure).
- 🟡 medium (~½ day, including ui-build setup).

### P3 — Cell-edit: forgetting which fields are required → silent save with missing data

**Pain:** Tabularis NewRowModal shows "REQUIRED" placeholder but doesn't block submit. Plugin now catches it post-save, but the error appears AFTER the modal closes — confusing.

**Want:** Pre-submit highlight of empty required fields, modal stays open.

**Path:**
- Tabularis upstream issue (already filed).
- UI-Extension workaround: `row-edit-modal.footer.before` slot that runs the validation client-side and shows inline.

**Effort:** 🟢 (UI-Extension workaround ~2 h).

### P4 — Editing a `reference` field means manually copy-pasting `users/abc123` doc-paths

**Pain:** No autocomplete. Have to pre-fetch the doc-id, type/paste it into the cell.

**Want:** Click → dropdown of valid candidates from the referenced collection.

**Path:**
- UI-Extension on `row-edit-modal.field.after` for `reference`-type fields, dropdown populated via `usePluginQuery`.

**Effort:** 🟡 (~½ day).

---

## Multi-row operations (encountered ≥ once per week)

### P5 — Bulk-delete: "delete all 47 inactive users"

**Pain:** Today: select rows manually one at a time. Or write a Firebase Admin script.

**Want:** "Delete all matching" button on a filtered grid.

**Path:**
- New RPC `bulk_delete` that takes a where-clause and paginates the delete (Firestore ≤ 500 ops per commit).
- UI-Extension on `data-grid.toolbar.actions`: "Delete all N matching" with confirmation.

**Effort:** 🟡 (~½ day for both).

### P6 — Bulk-insert: import a CSV/JSON file into a collection

**Pain:** Either write a one-off script or click "+" 50 times.

**Want:** Drop a file → preview → confirm → done.

**Path:**
- New RPC `bulk_insert` with chunked Firestore writes.
- UI-Extension toolbar button + drop-zone modal via `usePluginModal`.

**Effort:** 🟡 (~½ day).

### P7 — Export current view to NDJSON or CSV

**Pain:** Tabularis "Export" is generic but doesn't know about Firestore's nested types, so map/array columns export as `[object Object]`. Need explicit Firestore-aware export.

**Want:** Toolbar button that exports the current query results as proper JSON.

**Path:**
- UI-Extension on `data-grid.toolbar.actions` — fetch via `usePluginQuery` (or directly invoke `execute_query`), parse the stringified maps/arrays, emit clean JSON.

**Effort:** 🟢 (~2 h).

---

## Investigation / debugging (encountered ad-hoc)

### P8 — "Why is this query slow?"

**Pain:** The EXPLAIN plan exists but doesn't tell the user *which index would help*. Just "no index used → fallback scan".

**Want:** Suggested composite-index definition (field paths + directions) so user can paste into `firestore.indexes.json`.

**Path:**
- Plugin already extracts the composite-index URL when Firestore returns one. Extend to also surface the field paths in a copy-paste-ready block.
- Plus: warn before running if we can predict the missing index (parse + check against known indexes).

**Effort:** 🟢 surface URL + paths · 🟡 predictive warning.

### P9 — "What changed in this doc?"

**Pain:** No history view. Have to query Firestore Admin API or check Cloud Audit Logs externally.

**Want:** Right-click → "Show audit history" → list of writes with timestamps.

**Path:**
- Firestore has no built-in doc-versioning. Three options:
  - Cloud Audit Logs (requires GCP project setup — opt-in).
  - User-side `__history` subcollection (app must populate; we just read).
  - "Snapshot now" button that writes the doc to a `__snapshots/<table>/<doc>/<timestamp>` path manually.

**Effort:** 🔴 across all options. Probably skip unless feature requested explicitly.

### P10 — "Where else is this doc referenced?"

**Pain:** Need to know all docs that have `author = users/alice` before deleting a user. Have to grep manually.

**Want:** Right-click on a doc → "Find references" → table of incoming references.

**Path:**
- Walk every collection's reference-fields (we have this metadata from `get_schema_snapshot`), run `WHERE <field> = '<doc-path>'` queries in parallel.
- UI-Extension `data-grid.context-menu.items`.

**Effort:** 🟡 (~½ day, mostly polishing the result presentation).

---

## Schema management (encountered when modeling new features)

### P11 — Hand-editing the schema-overrides JSON for new collections

**Pain:** We just shipped schema-overrides via JSON file. Editing it is fine for one-off changes; for a new collection with 15 fields, it's tedious.

**Want:** A schema-editor modal: pick collection → form per field (required toggle, type dropdown, comment) → write file.

**Path:**
- UI-Extension that opens a `usePluginModal` with the editor.
- New RPC `read_schema_overrides` + `write_schema_overrides` to round-trip the file.

**Effort:** 🔴 (~1 day, but its own self-contained feature).

### P12 — Discovering a field exists when it's not in the sample

**Pain:** Schema-inference samples 50 docs. If a field is set on doc 51+, it's invisible until you sample more — but the user has to know to bump `sample_size`.

**Want:** Either a "scan all docs for fields" action, or surface the count ("inferred from 50/847 docs — 3 fields may be hiding").

**Path:**
- Cheap fix: include `sample_size` and `total_count` in the schema-snapshot response, surface in column tooltips.
- Expensive fix: full-scan inference button.

**Effort:** 🟢 cheap fix.

---

## Top picks for "next 1-2 sessions" power-user batch

If you wanted to hit 5-6 daily-driver wins in a single push:

1. **P1 short-term**: Settings reload-able (`OnceCell` → `RwLock`) — 1 h, unblocks dev↔prod without restart even before Tabularis upstream
2. **P7 NDJSON Export** — 2 h, toolbar button — biggest "I can finally get my data out" win
3. **P5/P6 Bulk operations** — ½ day for both — turns the plugin from "viewer" into "ops tool"
4. **P3 inline required-field validation** — 2 h UI-Extension workaround
5. **P12 cheap fix**: surface sample-size hint — 30 min

Total ~1.5 days. Material upgrade in daily UX.

---

## Things explicitly NOT in scope

- Multi-tenant authorization / row-level security
- Schema migrations (Firestore is schemaless; "migration" = bulk update doc shape)
- Full-text search beyond what Firestore native provides
- Backup / restore (Firestore Admin export → GCS is the canonical path; not our job)
- Performance profiling beyond EXPLAIN

These are out-of-scope because Firestore-the-product handles them differently than relational DBs and the plugin shouldn't pretend otherwise.
