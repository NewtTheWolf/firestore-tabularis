# Todo / Followups

Plain index of stuff we want to come back to. Each entry is one line; if something needs design, link to a spec under `docs/superpowers/specs/`.

## Upstream (Tabularis core)

- [ ] **File Tabularis issue: plugin-manifest `icon` field** — add `icon` (relative path to PNG/SVG inside plugin folder) to `plugins/manifest.schema.json` so external drivers can ship custom icons. Schema currently has `additionalProperties: false`, so this needs a Tabularis-side PR. Repo: `TabularisDB/tabularis`.
- [ ] **Watch Tabularis #24 — JSON/JSONB Editor & Viewer** (https://github.com/TabularisDB/tabularis/issues/24). When this ships, revert the Phase-2 stringification (commit `63e0912`) and re-enable native JSON values for Map/Array cells; we may also need to expose a `JSON` entry in `manifest.json:data_types` and have `schema_infer` emit `data_type: "json"` for Map/Array columns so the editor's selector triggers. The current `[object Object]` hover bug is a symptom of the missing JSON renderer this issue introduces.

## firestore-driver (this repo)

- [ ] Real-Firestore smoke tests against `luninora` per Phase 2 plan Step 7 (manual gate — the items in `docs/superpowers/plans/2026-05-08-phase-2-firestore-query-layer.md` § Task 16 § Step 7).
- [ ] Phase 3 brainstorm: CRUD (insert / update / delete record). See `docs/ROADMAP.md` Phase 3.
