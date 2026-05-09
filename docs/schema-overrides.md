# Schema Overrides

Firestore is schemaless — every field in every document may legitimately be
absent. The plugin defaults to `is_nullable: true` for all non-`id` fields,
which matches Firestore semantics but discards information power users care
about: which fields are *conceptually* required, which are misclassified by
sample-driven inference, which should be hidden from the grid.

The schema-overrides system lets you declare those properties per
collection in a JSON file. Tabularis reads them as part of the column
metadata, so saves get blocked on missing required fields, types display
correctly in the grid, hidden fields disappear, and tooltips carry your
documentation.

## Setup

1. Create a directory anywhere — `~/firestore-schemas/` or a path inside
   your repo so it can be checked into git for the team.
2. In Tabularis: **Plugin Settings → Firestore → Schema Overrides Directory**,
   point to the directory.
3. Drop a JSON file in there named for your project + database.

### Filename lookup

The plugin looks up files in this order, returning the first match:

1. `{project_id}_{database_id}.json` — most specific
2. `{project_id}.json` — fallback for single-database setups

The `(default)` parens in the standard database id are stripped, so the
common case becomes `myproject_default.json`.

Examples:
- Project `luninora-dev`, default DB → `luninora-dev_default.json` or
  just `luninora-dev.json`
- Project `acme`, custom DB `analytics` → `acme_analytics.json`

### Reload

Files are loaded once at plugin init. To pick up edits, **toggle the
connection** (disconnect + reconnect) in Tabularis.

## File format

```jsonc
{
  "$schema": "https://tabularis.dev/firestore-plugin/schema-overrides.v1.json",
  "collections": {
    "advisors": {
      "fields": {
        "email":         { "required": true, "comment": "Login identifier" },
        "firstName":     { "required": true },
        "rating":        { "type": "number", "required": false },
        "internalNotes": { "hidden": true }
      },
      "extra_fields": {
        "rarelySetField": { "type": "string", "required": false, "comment": "Optional details" }
      }
    },
    "users": {
      "fields": {
        "email": { "required": true },
        "uid":   { "required": true, "comment": "Firebase Auth UID" }
      }
    }
  }
}
```

### `fields` — override behavior of fields the inference already knows

| Property | Type | Effect |
|---|---|---|
| `required` | `bool` | `true` flips `is_nullable` to `false` (Tabularis blocks save when empty). `false` forces nullable even if your inference suggests otherwise. |
| `type` | `string` | Overrides the inferred `data_type`. Useful when sample-driven inference reports `mixed`. Allowed values: `string`, `number`, `boolean`, `timestamp`, `binary`, `geopoint`, `reference`, `array`, `map`, `null`, `mixed`. |
| `hidden` | `bool` | Drops the column from the grid entirely. Useful for internal/audit fields. |
| `comment` | `string` | Free-form description; Tabularis shows it in the column tooltip. |

### `extra_fields` — declare fields not present in the sample

Same property set as `fields`. Useful for:
- New collections where the seed doc is incomplete
- Optional/rare fields the schema sample missed
- Documenting expected-but-not-yet-used fields

## Validation

Type values are validated when the plugin starts. An unknown type aborts
the connection with a clear error like:

```
schema_overrides: collection 'advisors' field 'rating' has unknown type 'fnord'.
Allowed: string, number, boolean, timestamp, binary, geopoint, reference, array, map, null, mixed
```

Invalid JSON likewise surfaces immediately rather than mysteriously
discarding the file.

## Limitations

- **Currently global per plugin install** — Tabularis doesn't yet expose a
  per-connection driver-specific settings bag. If you have two connections
  (e.g. dev + prod), the schema-overrides directory is shared. The
  filename lookup mitigates this for the common case (different
  `{project}_{db}.json` files for each), but the plugin still reads only
  the file matching the *currently active* settings. Phase 4 will lift
  this restriction via a Tabularis upstream change.
- **No live reload** — edits require a connection toggle. Filesystem
  watching has too many edge cases for too little benefit; the toggle
  takes one click.
- **Inheritance / wildcards** — collection-level overrides are exact-match
  only. No `users_*` patterns or "apply to all collections" rules. Add
  per-collection blocks where needed.
