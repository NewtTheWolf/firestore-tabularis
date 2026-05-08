# Todo / Followups

Plain index of stuff we want to come back to. Each entry is one line; if something needs design, link to a spec under `docs/superpowers/specs/`.

## Upstream (Tabularis core)

- [ ] **File Tabularis issue: plugin-manifest `icon` field** — add `icon` (relative path to PNG/SVG inside plugin folder) to `plugins/manifest.schema.json` so external drivers can ship custom icons. Schema currently has `additionalProperties: false`, so this needs a Tabularis-side PR. Repo: `TabularisDB/tabularis`.
- [ ] **File Tabularis issue: data-grid hover renders nested JSON as `[object Object]`** — when a row payload contains a native JSON object/array, the cell tooltip uses JS default coercion instead of `JSON.stringify`. We worked around it in firestore-driver by JSON-stringifying Maps/Arrays (Phase 2 revert). Once fixed upstream, we can re-enable native JSON values for richer rendering (see also Phase 4 data-grid UI extension).

## firestore-driver (this repo)

- [ ] Real-Firestore smoke tests against `luninora` per Phase 2 plan Step 7 (manual gate — the items in `docs/superpowers/plans/2026-05-08-phase-2-firestore-query-layer.md` § Task 16 § Step 7).
- [ ] Phase 3 brainstorm: CRUD (insert / update / delete record). See `docs/ROADMAP.md` Phase 3.
