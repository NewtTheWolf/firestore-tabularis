# Brainstorm: UI Extensions — Tabularis slot inventory

**Date:** 2026-05-09
**Lens:** Concrete things we could build with Tabularis' 10 UI-extension slots. One section per slot. Each idea lists the user-facing value, the effort, and the dependencies.

**Manifest declaration shape** (for context):
```json
{
  "ui_extensions": [
    {
      "slot": "data-grid.context-menu.items",
      "module": "./ui/dist/index.js",
      "order": 100,
      "driver": "firestore"
    }
  ]
}
```
Bundle is React-IIFE; Tabularis injects React + JSXRuntime + `__TABULARIS_API__`. Our UI uses `usePluginQuery`, `usePluginConnection`, `usePluginToast`, `usePluginSetting`, `usePluginModal`, `usePluginTheme`, `usePluginTranslation`, `openUrl`. Writes go via `tauri.invoke('insert_record', ...)` directly.

---

## Slot 1 — `connection-modal.connection_content`

Renders inside the New-Connection modal. Slot context: `{ driver, database, onDatabaseChange, connectionName }`.

| | Idea | Effort | Blocker |
|---|---|---|---|
| C1 | **Per-connection project_id / database_id / SA-path / emulator-host fields** with autocomplete from `gcloud projects list` | 🔴 | ⛔ Tabularis `driver_specific` HashMap missing |
| C2 | "Pick service-account JSON" file picker → fills SA-path field | 🟢 | ⛔ same |
| C3 | "Use Emulator" toggle that conditionally reveals the emulator-host input | 🟢 | ⛔ same |
| C4 | Live ADC-status indicator: "logged in as alice@example.com" / "no ADC" with "Login with gcloud" button | 🟡 | ⛔ same |

→ All blocked on the same upstream. File the issue, then do them as one cohesive UI.

## Slot 2 — `settings.plugin.before_settings`

Renders above the plugin-settings form (when user opens Plugin Settings → Firestore). No connection context required.

| | Idea | Effort | Blocker |
|---|---|---|---|
| SB1 | ADC-status indicator (same as C4 but plugin-global) | 🟢 | 🟦 |
| SB2 | "Test connection now" button that invokes `test_connection` and shows the result inline | 🟢 | 🟦 |
| SB3 | Schema-overrides directory picker + "Open in editor" button | 🟢 | 🟦 |
| SB4 | "Refresh schema cache" button (invalidates SCHEMA_CACHE) | 🟢 | 🟦 (needs new RPC `refresh_schema_cache`) |

→ Easy wins. Probably ship as a single mini-dashboard above the settings form.

## Slot 3 — `settings.plugin.actions`

Action-buttons row in plugin-settings.

| | Idea | Effort | Blocker |
|---|---|---|---|
| SA1 | "Validate schema-overrides file" button — parse + report errors before reconnecting | 🟢 | 🟦 |
| SA2 | "Export schema as TypeScript types" — generate `interface Advisor {…}` from `get_schema_snapshot` | 🟡 | 🟦 |
| SA3 | "Import indexes" — read `firestore.indexes.json`, surface in Tabularis as advisory metadata | 🟡 | 🟦 |

## Slot 4 — `data-grid.toolbar.actions`

Button row in the data-grid toolbar. Slot context includes `tableName`.

| | Idea | Effort | Blocker |
|---|---|---|---|
| TB1 | **"Export to NDJSON"** — current query's results → file download | 🟡 | 🟦 |
| TB2 | "Export to CSV" — flat (filters out maps/arrays unless coerced) | 🟢 | 🟦 |
| TB3 | "Bulk Insert from JSON" — drop-zone modal | 🟡 | 🟦 |
| TB4 | **"Live Mode" toggle** — Firestore listener streams updates into the grid | 🔴 | ⛔ Tabularis JSON-RPC must accept server-initiated frames |
| TB5 | "Aggregate" dropdown — COUNT/SUM/AVG over current filter | 🟡 | 🟦 (depends on Q2 in holistic) |
| TB6 | "Open in Firebase Console" — collection link | 🟢 | 🟦 |

## Slot 5 — `data-grid.context-menu.items`

Right-click menu on grid cells/rows. Slot context: `tableName, columnName, rowData, rowIndex`.

| | Idea | Effort | Blocker |
|---|---|---|---|
| CM1 | **"View as JSON tree"** for map/array cells (workaround for `[object Object]` hover until Tabularis #24 ships) | 🟡 | 🟦 |
| CM2 | **"Open in Firebase Console"** — doc-level link | 🟢 | 🟦 |
| CM3 | **"Follow reference"** — clicking on a `users/abc123`-cell jumps to the `users` table filtered to that doc | 🟢 | 🟦 |
| CM4 | "Copy as JSON" — full doc as clipboard text | 🟢 | 🟦 |
| CM5 | "Copy doc-id" — just the synthetic id field | 🟢 | 🟦 |
| CM6 | "Duplicate document" — new doc with same fields, autogen id | 🟢 | 🟦 |
| CM7 | "Show audit history" — Firestore changelog API (if available on the project) | 🔴 | 🟦 + GCP project Firestore Audit Logs feature flag |
| CM8 | "Compare with sibling" — pick another doc, diff the fields | 🟡 | 🟦 |

## Slot 6 — `row-edit-modal.field.after`

Per-field slot in the insert modal. Context includes `columnName, dataType, isInsertion`.

| | Idea | Effort | Blocker |
|---|---|---|---|
| RE1 | **Reference-picker** for `reference`-typed fields — dropdown listing docs of the target collection | 🟡 | 🟦 |
| RE2 | Timestamp helper — "Now" button + relative-time chips ("1h ago", "yesterday") | 🟢 | 🟦 |
| RE3 | GeoPoint picker (mini-map) for `geopoint` fields | 🔴 | 🟦 + map library |
| RE4 | Auto-generated UUID button for string-id fields | 🟢 | 🟦 |
| RE5 | Server-side timestamp toggle ("set to serverTimestamp() on save") | 🟡 | 🟦 (sentinel value handling in coercion) |

## Slot 7 — `row-edit-modal.footer.before`

Modal footer (before the action buttons).

| | Idea | Effort | Blocker |
|---|---|---|---|
| RF1 | Pre-validation feedback ("3 required fields not set") — beats the post-RPC error | 🟢 | 🟦 |
| RF2 | "Save and add another" button (clears form after save) | 🟢 | 🟦 (depends on Tabularis form-state API) |

## Slot 8 — `row-editor-sidebar.field.after` / `row-editor-sidebar.header.actions`

Same shape as the modal slots but in the inline-edit sidebar. Most ideas there port over (RE1–RE5, RF1–RF2).

## Slot 9 — `sidebar.footer.actions`

Buttons in the sidebar footer (under the table list).

| | Idea | Effort | Blocker |
|---|---|---|---|
| SF1 | "Refresh schemas" — re-runs `get_schema_snapshot`, picks up new collections | 🟢 | 🟦 |
| SF2 | "Index health" — quick view of which queries hit composite indexes vs not | 🟡 | 🟦 |
| SF3 | "Storage estimate" — rough doc-count + size per collection | 🟡 | 🟦 (Firestore Admin API) |

## Slot 10 — Modals via `usePluginModal`

Not a slot per se; opens a custom modal from any of the slots above.

| | Idea | Effort | Blocker |
|---|---|---|---|
| M1 | **Schema-overrides editor** (per-collection field config UI) — opens from settings or sidebar | 🔴 | 🟦 |
| M2 | "New collection" wizard (Firestore creates implicitly, but the wizard pre-fills the seed-doc fields) | 🟡 | 🟦 |
| M3 | "Bulk operation" preview modal (delete-N or insert-N with confirmation) | 🟡 | 🟦 |
| M4 | "Query builder" — visual filter constructor for users who don't write SQL | 🔴 | 🟦 |

---

## Build-pipeline gap

We don't have `ui/` in the repo yet. Bringing it online means:

```
ui/
  package.json           # react peer-dep, vite, typescript
  vite.config.ts         # IIFE output, externalised React
  src/
    index.tsx            # defineSlot(...) calls
    components/
      OpenInConsole.tsx
      JsonTreeView.tsx
      FollowReference.tsx
      ...
  dist/index.js          # bundled output (gitignored, built into installation)
```

`just ui build` is wired but no-op until `ui/package.json` exists. First UI-extension work session would set up Vite + a hello-world IIFE bundle, *then* layer the first feature on top.

---

## Top picks (lowest effort × highest visible value)

1. **CM2 + CM3 + CM6** ("Firebase Console", "Follow reference", "Duplicate") — together one right-click-menu extension, ~2-3 h total. Daily-driver wins.
2. **CM1 JSON tree-view** — fixes the `[object Object]` hover gap until Tabularis #24 ships.
3. **SB2 "Test connection now"** + **SB1 ADC-status** in settings panel — onboarding polish.
4. **TB1 NDJSON export** — power-user export that's missing today.
5. **RE1 Reference-picker** — fixes the "type a doc-id from another collection" pain.
