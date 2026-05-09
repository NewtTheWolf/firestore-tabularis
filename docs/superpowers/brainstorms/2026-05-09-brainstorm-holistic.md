# Brainstorm: Holistic — Plugin opportunity map

**Date:** 2026-05-09
**Lens:** Everything the firestore-driver could become. Tabularis-UI extensions, backend features, power-user tooling, Phase-4 themes, ops. A wide-cast catalog to pick from.

**Notation:**
- **Effort:** 🟢 quick (≤ 2 h) · 🟡 medium (½–1 day) · 🔴 large (multi-day)
- **Blocker:** ⛔ requires Tabularis upstream change · 🟦 ours alone

---

## 1. Query / Read layer

| | What | Effort | Blocker | Notes |
|---|---|---|---|---|
| Q1 | `sqlparser` migration | 🟡 | 🟦 | Already on the next-session priority list. Unlocks DML SQL + reduces parser maintenance. |
| Q2 | `COUNT(field IS NOT NULL)` aggregations | 🟢 | 🟦 | Firestore aggregation API supports COUNT/SUM/AVG. Wire into the parser. |
| Q3 | Cursor-page-info in response (`has_next_page`, `next_page_token`) | 🟢 | 🟦 | We already build cursors internally. Surface them so Tabularis pagination doesn't re-issue OFFSETs. |
| Q4 | `LIMIT` cap with structured warning ("LIMIT 10000 capped to 1000, click X to fetch all") | 🟢 | 🟦 | Defends against accidental full-collection scans. |
| Q5 | Composite-index suggestions: not just the URL, but the field-paths so user can paste into `firestore.indexes.json` | 🟢 | 🟦 | Stretch already on todo. |
| Q6 | Query templates / saved queries per connection | 🟡 | ⛔ for true per-connection storage; 🟦 for per-plugin-install | Stretch goal in roadmap. |

## 2. Write layer

| | What | Effort | Blocker | Notes |
|---|---|---|---|---|
| W1 | Bulk insert via CSV/JSON drop | 🟡 | 🟦 | New RPC `bulk_insert` with 500/batch chunking (Firestore limit). UI-Extension on toolbar. |
| W2 | Bulk delete from query result | 🟡 | 🟦 | "Delete all 47 matching rows" — server-side delete by query, paginated. |
| W3 | Optimistic concurrency via `update_time` precondition | 🟡 | ⛔ Tabularis doesn't surface update_time today | Last-writer-wins is current behavior; Phase 4 candidate. |
| W4 | Atomic field operations: `INCREMENT(views, 1)`, `ARRAY_UNION(tags, 'x')` | 🟡 | 🟦 (parser); ⛔ for UI exposure | Firestore-native operations not expressible in SQL. Could expose via a special syntax or a context-menu action. |
| W5 | Document `merge` semantics toggle (currently we PATCH = full overwrite of provided fields) | 🟢 | 🟦 | Plugin-setting + per-call override. |

## 3. Schema / Inference

| | What | Effort | Blocker | Notes |
|---|---|---|---|---|
| S1 | Schema-overrides editor UI (instead of editing JSON by hand) | 🔴 | 🟦 (own modal) | Worth its own design. UI extension that opens a per-collection schema editor; writes the override file. |
| S6 | **Schemaful Mode** — opt-in spectrum from pure Firestore to strict schemaful via definition docs (in-collection or sibling layout), three-layer resolution, optional cascading field-delete | 🔴 | 🟦 | See [`brainstorm-schemaful-mode.md`](./2026-05-09-brainstorm-schemaful-mode.md). Subsumes S1 (the schema editor) plus solves "how do I create a collection from Tabularis". |
| S2 | Schema-cache TTL + manual "Refresh schema" action | 🟢 | 🟦 | On the stretch list. Today the cache lives for the plugin process; external writes to the field shape stay invisible until restart. |
| S3 | Confidence indicator per inferred type ("100% string in 50 sampled docs", "mixed: 30% int / 70% string") | 🟢 | 🟦 | Surfaces in column tooltips. Helps user know which fields they should override. |
| S4 | Inference of map-shape (recursive: nested fields shown as dot-paths) | 🟡 | 🟦 (driver); ⛔ Tabularis would need to render dot-path columns | Currently we collapse maps to one "map" type — losing nested structure. |
| S5 | Reference-validity check ("47 docs reference users/abc123 which doesn't exist") | 🟡 | 🟦 | Schema-snapshot pass. Big perf cost on large databases — opt-in. |

## 4. UI / UX (deep dive in `2026-05-09-brainstorm-ui-extensions.md`)

| | What | Effort | Blocker | Notes |
|---|---|---|---|---|
| U1 | Per-connection settings via `connection-modal.connection_content` slot | 🔴 | ⛔ Tabularis `driver_specific` HashMap | Phase-4 enabler. |
| U2 | "Open in Firebase Console" right-click action | 🟢 | 🟦 | Trivially valuable. |
| U3 | Map/Array tree-view in right-click menu | 🟡 | 🟦 (until Tabularis #24 ships) | Workaround for the `[object Object]` hover bug. |
| U4 | Reference-picker on insert/edit forms | 🟡 | 🟦 | Pick a doc-id from a referenced collection instead of typing it. |
| U5 | ADC / login-status indicator + "Test connection now" button | 🟢 | 🟦 | Settings-slot polish. |

## 5. Phase 4 / Multi-X

| | What | Effort | Blocker | Notes |
|---|---|---|---|---|
| M1 | Multi-database per project (named DBs beyond `(default)`) | 🟡 | ⛔ for per-connection routing; 🟦 for per-plugin-install | `state::CLIENT` becomes `HashMap<(project, db), FirestoreDb>`. |
| M2 | Multi-project per Tabularis install | 🔴 | ⛔ Tabularis `driver_specific` | Each connection has its own project_id. The dev↔prod-without-restart fix. |
| M3 | Subcollections — three options in ROADMAP (hierarchical sidebar / pseudo-table naming / schema-snapshot only) | 🔴 | ⛔ for sidebar option; 🟦 for the others | Hardest mapping. Probably start with schema-snapshot only. |
| M4 | Real-time listener "Live mode" toggle on grid | 🟡 | ⛔ Tabularis JSON-RPC bridge must accept server-initiated frames | Verify first; could be huge UX win for ops dashboards. |

## 6. Tooling / DevX

| | What | Effort | Blocker | Notes |
|---|---|---|---|---|
| T1 | GitHub Action: `just emulator test` on every PR | 🟢 | 🟦 | Java 21 + bun via setup-actions. ~15s warm. |
| T2 | Trait `FirestoreOps` + `mockall` for handler unit-tests | 🔴 | 🟦 | Brings handlers/metadata.rs from 0% to ~90%. Worth it iff metadata bugs slip through. |
| T3 | Property-based parser tests via `proptest` | 🟡 | 🟦 | Probably obviated by sqlparser migration. |
| T4 | Visual changelog page (`docs/CHANGELOG.md`) | 🟢 | 🟦 | We already write good commit messages — generate from git log. |
| T5 | Plugin-protocol fuzz tests (random JSON-RPC frames into stdin) | 🟡 | 🟦 | Catches dispatcher panics on malformed input. |

## 7. Observability

| | What | Effort | Blocker | Notes |
|---|---|---|---|---|
| O1 | Replace `eprintln!` with `tracing` (JSON output to stderr) | 🟡 | 🟦 | Captured by Tabularis stderr; user can pipe to a log analyzer. |
| O2 | Per-call latency metrics in response (`firestore_round_trip_ms`) | 🟢 | 🟦 | Helps debug slow queries. |
| O3 | Sampled query log to disk for "what did I run last week" | 🟡 | 🟦 | Plugin-setting toggle, capped file size. |

## 8. Distribution

| | What | Effort | Blocker | Notes |
|---|---|---|---|---|
| D1 | Plugin registry PR (Tabularis in-app store) | 🟢 | 🟦 (after our v1.0 cuts) | Roadmap Phase 5 item. |
| D2 | Pre-built bundles: Linux x64/arm64 + macOS x64/arm64 + Windows x64 | 🟢 | 🟦 | Existing `release.yml` workflow handles this already; just needs a tag. |
| D3 | Plugin-icon support in manifest (currently blocked by Tabularis schema) | 🟢 | ⛔ | Already on todo. |

---

## Top picks (highest leverage)

1. **Q1 sqlparser** — already next-session pick. Removes a 1100-LOC maintenance liability.
2. **U2 "Open in Firebase Console"** — 1 hour, every Tabularis user benefits.
3. **W1/W2 Bulk operations** — single biggest power-user productivity win. ½ day for both.
4. **S1 Schema-overrides editor UI** — reduces friction on the schema-overrides feature we just shipped.
5. **T1 GitHub Action** — locks in test discipline for future work.

## Blocked-on-Tabularis-Upstream

These can't move until upstream lands:
- M2 Multi-project per connection ← `driver_specific` HashMap
- U1 Per-connection settings UI ← same
- D3 Plugin-icon ← manifest schema
- M4 Live-mode listener ← server-initiated JSON-RPC frames

File the upstream issues (already noted in `tasks/todo.md`).
