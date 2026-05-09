# Brainstorm: Schemaful Mode — opt-in schema discipline for Firestore

**Date:** 2026-05-09
**Lens:** Let users opt into varying degrees of schema discipline without losing Firestore's schemaless flexibility. The plugin offers a spectrum from "pure Firestore" to "strict schemaful". Each step is opt-in.

---

## The spectrum

| Mode | What user does | What plugin enforces |
|---|---|---|
| **Pure Firestore** | nothing | sample-based inference only; no constraints |
| **Loose schemaful** | writes a local schema-overrides JSON file | required-fields + type hints applied to UI; no destructive ops |
| **Team schemaful** | writes a `_schemas/<collection>` definition doc in Firestore itself | same as loose, but the definition is shared with all clients of the project |
| **Strict schemaful** | sets `enforcement: strict` on the definition or override | removing a field from the definition cascades the delete across all docs |

Default everywhere: most permissive. User opts in step-by-step.

---

## Three-layer schema resolution

When the plugin builds the column list for a collection (in `get_columns` / `get_schema_snapshot`), it merges three sources in this precedence order:

```
1. Local schema-overrides file (~/firestore-schemas/<project>_<db>.json)   ← wins
2. Server-side definition doc  (location depends on layout setting)        ← middle
3. Sample-based inference      (the docs themselves)                       ← fallback
```

Each layer can:
- **Add** fields (extra_fields)
- **Override** field properties (required, type, comment, hidden)
- **Hide** fields entirely

Lower layers fill in what higher layers don't specify. The local file wins so a developer can experiment without touching the team's shared definition.

---

## Component 1: The Definition Doc

The plugin supports **two layouts** for where the definition doc lives. User picks at install time via plugin setting `schema_doc_layout`:

### Layout A: `in-collection` (default)

The definition doc lives **inside the collection it describes**, at a reserved doc-id:

```
advisors/_schema
test/_schema
users/_schema
```

**Pro:** discoverable (the schema lives with the data), no special collection to manage, Firestore IAM rules at collection level apply to schema reads/writes the same as data reads/writes.
**Con:** must filter `_schema` doc out of every query result, total_count, inference sample, and id-list — five filter points.

### Layout B: `sibling-collection`

The definition lives in a sibling collection, one doc per real collection:

```
_schemas/advisors
_schemas/test
_schemas/users
```

**Pro:** clean separation of metadata vs data, one place to find all schemas, can have separate IAM rules ("who is allowed to edit schemas?").
**Con:** must hide `_schemas` from `get_tables` and `get_schema_snapshot`, plus an extra round-trip to fetch schemas (vs co-located in collection).

### Shared shape (regardless of layout)

```json
{
  "fields": [
    { "name": "email",     "type": "string",    "required": true, "comment": "Login identifier" },
    { "name": "rating",    "type": "number",    "required": false },
    { "name": "createdAt", "type": "timestamp", "required": true }
  ],
  "enforcement": "advisory",
  "updated_at": "2026-05-09T22:00:00Z",
  "updated_by": "tabularis-firestore-plugin"
}
```

### Setting summary

```
schema_doc_layout: "in-collection" | "sibling-collection"   (default: in-collection)
schema_doc_id:     "_schema"                                 (used by in-collection layout; configurable to avoid collision)
schema_collection: "_schemas"                                (used by sibling-collection layout; configurable too)
```

If a real document with id `_schema` (or whatever's configured) already exists and **doesn't have the expected shape** (missing `fields` array), the plugin treats it as a regular doc and skips schemaful behavior for that collection — with a warning surfaced via `get_columns`.

**Ownership decision (v1):** plugin writes it, app code ignores it. Tabularis is the single editor. Cleanest for v1; if app-code-driven schemas become a use case, we revisit (last-write-wins or merge strategy).

---

## Component 2: Footer wizard "+ New Collection"

UI extension on `sidebar.footer.actions`:

1. User clicks `+ New Collection`
2. Modal asks for: collection name, list of fields (name + type + required toggle), optional first doc
3. On submit, plugin writes (in this order, depending on the active layout):
   - the definition doc at `<collection>/_schema` (layout A) or `_schemas/<collection>` (layout B)
   - `<collection>/<seed-doc-id>` first data doc (auto-generated id if not provided)
4. Sidebar refreshes — new collection appears

The same modal serves "Edit Schema" via right-click on an existing collection — pre-populated from the existing `_schemas/<coll>` doc.

**Cascade hook:** if the user removes a field while editing an existing schema and the enforcement is `strict`, the modal warns "X docs have this field — remove from all?". Confirmation triggers the cascading delete.

---

## Component 3: Strict-mode field deletion

When `enforcement: "strict"` is active for a collection AND a field is removed from the definition:

1. Plugin scans all docs in the collection (paginated, 500/batch).
2. For each doc that has the removed field, issues an update with `FieldValue.delete()` sentinel for that field.
3. Logs the operation (write count + duration) so user sees what happened.

Failures: best-effort. If a delete fails mid-stream (network blip, permission denied), surface the error with "Y of X docs cleaned up" so user can rerun.

**Safety rails:**
- Confirmation dialog: "Remove field `notes` from 4,827 docs? This cannot be undone."
- Disabled when collection has > 10,000 docs (configurable threshold) — at that scale, do it as a Firestore Admin batch job, not in-band.
- Definition-doc updates are atomic-ish via Firestore's update_doc (single PATCH); the cascade is a separate non-atomic step.

---

## Component 4: Configuration hierarchy for `enforcement`

Most-specific wins:

```
1. <definition-doc>.enforcement               ← per-collection (in either layout)
2. <project>_<db>.json: default_enforcement   ← per-(project, db) in override file
3. manifest.json: default_enforcement         ← per-plugin-install
```

Default at every level: `advisory`. User has to flip to `strict` explicitly somewhere.

---

## Component 5: Documentation (`docs/firestore-as-schemaful.md`)

Explains the spectrum so users can pick their level. Roughly:

> Firestore is schemaless by design — but in practice every team has a de-facto schema. This plugin lets you make it explicit, in degrees:
>
> 1. **Pure Firestore**: do nothing. The plugin samples docs and infers types. Default.
> 2. **Loose schemaful**: drop a JSON file in `~/firestore-schemas/`. Required fields, type corrections, hidden columns, all as a local config. Per-developer.
> 3. **Team schemaful**: click `+ New Collection` in Tabularis (or write `_schemas/<coll>` from your app). The schema lives in Firestore itself, shared by anyone connecting to the project.
> 4. **Strict schemaful**: set `enforcement: "strict"` on a definition. Removing a field cascades the delete across all docs. Useful when migrating away from a deprecated field.
>
> Each step is opt-in. You can stop at any level.

---

## Open questions (parked for the design pass)

- **What happens if a doc-field disagrees with the definition?** (e.g. definition says `rating: number`, doc has `rating: "five"`). Options: tolerate (current behavior) / warn in column tooltip / refuse-on-edit. Default: tolerate + warn.
- **Inheritance for subcollections?** When Phase 4's subcollection support lands, does `_schemas/users/<sub>` apply to the subcollection? Probably yes, but TBD.
- **Migration story:** if a user has only a local schema-overrides file today and then switches on definition docs, do we offer a "promote local to team" button? Probably yes — would write the local config to `_schemas/<coll>` once per collection.
- **Tabularis `cascade_field_deletes` UX:** should the strict cascade run synchronously (block user) or as a background job? For ≤ 1000 docs, sync is fine; above that, suggest a background-job pattern.
- **Validation of the definition doc itself:** do we validate that field types are in the allowed set when reading? Yes — same validation as the schema-overrides file (we have it already).

---

## Implementation roadmap (rough)

| Phase | What | Effort |
|---|---|---|
| 1 | Read definition docs in `get_columns` (both layouts) + filter the schema doc/collection from queries / get_tables / inference | 🟡 ½–1 day (in-collection layout adds five filter points) |
| 2 | Manifest setting + plugin-side validation of definition docs | 🟢 1-2 h |
| 3 | UI extension `sidebar.footer.actions` "+ New Collection" wizard | 🔴 1-2 days (also bootstraps the `ui/` build pipeline) |
| 4 | "Edit Schema" right-click on collection → same wizard | 🟡 ½ day on top of phase 3 |
| 5 | Strict-mode cascading field-delete with confirmation | 🟡 ½ day |
| 6 | Docs (`docs/firestore-as-schemaful.md`) | 🟢 2 h |

Total: ~4-5 days for the full feature. Phases 1-2 alone are usable as "team schemaful read-only" without the wizard.

---

## Why this is a good design

- **Doesn't fight Firestore:** uses Firestore's own features (collections, docs, fields). No external metadata store.
- **Composable with existing schema-overrides:** the local file we just shipped is the most-specific layer. Existing users keep their workflow; new layer below it adds team-level sharing.
- **Progressive disclosure:** users can stop at any level. Pure-Firestore users see no change. Power users get strict mode.
- **Familiar UX:** "+ New Collection" mirrors what every relational tool offers — but the underlying model stays Firestore-native (collections are still implicit, the wizard just makes them explicit).
- **Reversible:** the definition doc is just data. Delete `_schemas/<coll>` and the plugin falls back to inference. No state machine.

## Why this might be a bad design (devil's advocate)

- **Two sources of truth:** local file + server doc. Users will get confused which one wins when. Mitigation: clear precedence in docs, plus the `usePluginQuery` could surface "schema source: definition doc" in the column tooltip.
- **The definition doc is just data:** in either layout, if a user has IAM that doesn't include write to it, schema edits fail with a permission error. Or worse — they have a real doc with id `_schema` already. Mitigation: structure validation (skip if doesn't look like a schema) + clear error messages + the configurable doc-id setting so collisions can be sidestepped.
- **Cascading field-delete is dangerous:** even with confirmation, users will hose their data. Maybe v1 ships without cascade and we add it later when we have more confidence in the UX.
