-- Full smoke-test corpus for the firestore-driver against luninora-dev.
-- Schema sampled 2026-05-09 via get_columns. Run these in Tabularis' Console
-- tab (one at a time) and verify the documented expectation.
--
-- Connection: luninora-dev / (default) / Firestore plugin
-- Plugin binary: target/release/firestore-plugin (or just dev-install)
--
-- Coverage status as of 2026-05-09: Phases 1, 2, 3 all shipped.
--
-- Legend
--   ✓ expected to succeed
--   ✗ expected to fail with a specific, structured error (validation/index)
--   ⚠ may fail on missing composite index — that is the test, not a bug
--   🔄 mutates Firestore state (Phase 3 — CRUD). Cleanup query at end of section.

===============================================================================
SECTION A — PHASE 2: QUERY LAYER (read-only)
===============================================================================

-------------------------------------------------------------------------------
-- A1. Basic SELECT / ORDER BY / LIMIT
-------------------------------------------------------------------------------

-- ✓ 1) Plain ORDER BY DESC
SELECT * FROM "advisors" ORDER BY createdAt DESC LIMIT 10;

-- ✓ 2) Equality + LIMIT (no composite index required)
SELECT * FROM "advisors" WHERE verified = true LIMIT 20;

-- ✓ 3) Boolean conjunction, all equality
SELECT * FROM "advisors" WHERE verified = true AND isAvailable = true LIMIT 20;

-- ✓ 4) Single-field range (Firestore allows ≤ 1 inequality field)
SELECT * FROM "advisors" WHERE rating >= 4.0 LIMIT 20;

-- ✓ 5) Range + ORDER BY on the same field
SELECT * FROM "advisors" WHERE rating >= 4.0 ORDER BY rating DESC LIMIT 10;

-------------------------------------------------------------------------------
-- A2. IN / NOT IN
-------------------------------------------------------------------------------

-- ✓ 6) IN list
SELECT * FROM "advisors" WHERE gender IN ('female', 'male', 'diverse') LIMIT 20;

-- ✓ 7) NOT IN list
SELECT * FROM "advisors" WHERE gender NOT IN ('male') LIMIT 20;

-------------------------------------------------------------------------------
-- A3. Document ID (synthetic `id` column → Firestore __name__ rewrite)
-------------------------------------------------------------------------------

-- ✓ 8) Direct doc-id lookup
SELECT * FROM "advisors" WHERE id = 'callservice';

-- ✓ 9) doc-id IN list (multi-key fetch)
SELECT * FROM "advisors" WHERE id IN ('callservice', '5mgfvqFS7QgxJvxAaqViiruxqy13');

-- ✓ 10) Plain string equality (verifies we still hit non-id strings)
SELECT * FROM "advisors" WHERE email = 'jan.poblenz+advisorverification2@luninora.de';

-------------------------------------------------------------------------------
-- A4. Array operators (Firestore-specific)
-------------------------------------------------------------------------------

-- ✓ 11) ARRAY_CONTAINS — single-value membership (infix form)
SELECT * FROM "advisors" WHERE products ARRAY_CONTAINS 'call' LIMIT 20;

-- ✓ 12) ARRAY_CONTAINS_ANY — multi-value membership (infix form)
SELECT * FROM "advisors" WHERE languagesSpoken ARRAY_CONTAINS_ANY ('de', 'en') LIMIT 20;

-- ✓ 12b) ARRAY_CONTAINS function form (alternative syntax)
SELECT * FROM "advisors" WHERE ARRAY_CONTAINS(products, 'call') LIMIT 20;

-------------------------------------------------------------------------------
-- A5. Timestamp literals
-------------------------------------------------------------------------------

-- ✓ 13) Timestamp range (open-ended)
SELECT * FROM "advisors"
  WHERE createdAt > TIMESTAMP '2026-01-01T00:00:00Z'
  ORDER BY createdAt DESC
  LIMIT 20;

-- ✓ 14) Timestamp bucket (closed range — same field both sides, no extra index)
SELECT * FROM "advisors"
  WHERE createdAt >= TIMESTAMP '2026-02-01T00:00:00Z'
    AND createdAt <  TIMESTAMP '2026-03-01T00:00:00Z'
  ORDER BY createdAt DESC;

-------------------------------------------------------------------------------
-- A6. OR / parens (Boolean tree)
-------------------------------------------------------------------------------

-- ✓ 15) OR — Firestore Filter.or under the hood
SELECT * FROM "advisors" WHERE verified = true OR emailVerified = true LIMIT 20;

-- ✓ 16) Parens override precedence
SELECT * FROM "advisors"
  WHERE (verified = true OR isListedByAdmin = true)
    AND productsEnabled = true
  LIMIT 20;

-------------------------------------------------------------------------------
-- A7. Pagination (cursor + count caches)
-------------------------------------------------------------------------------

-- ✓ 17a) Page 1 with LIMIT
SELECT * FROM "advisors" ORDER BY createdAt DESC LIMIT 5;

-- ✓ 17b) Page 2 — same query with OFFSET. Triggers OFFSET on cold cache,
--      switches to start_after() once page 1's cursor is in CURSOR_CACHE.
SELECT * FROM "advisors" ORDER BY createdAt DESC LIMIT 5 OFFSET 5;

-- ✓ 17c) Page 5 — exercises nearest-cursor fallback. If pages 1-3 were
--      visited, page 5 picks page-3's cursor and applies remainder OFFSET.
SELECT * FROM "advisors" ORDER BY createdAt DESC LIMIT 5 OFFSET 20;

-- ✓ 17d) Same query with HOST page params (mirrors what Tabularis sends from
--      the page-size selector). For a manual driver test, prefer 17a–17c.

-------------------------------------------------------------------------------
-- A8. Failure modes — these MUST return a structured error
-------------------------------------------------------------------------------

-- ⚠ 18) Composite-index trigger.
--      Inequality on `rating` + ORDER BY `createdAt` requires a composite
--      index. Expected: error message contains a Firebase Console URL.
SELECT * FROM "advisors" WHERE rating > 4 ORDER BY createdAt DESC LIMIT 10;

-- ✗ 19) Two inequality fields → caught by our pre-flight `validate()`
--      (Firestore rejects this; we catch before the round-trip).
SELECT * FROM "advisors" WHERE rating > 4 AND experienceYears > 5 LIMIT 10;

-- ✗ 20) ARRAY_CONTAINS + ARRAY_CONTAINS_ANY mix is illegal in Firestore
SELECT * FROM "advisors"
  WHERE products ARRAY_CONTAINS 'call'
    AND languagesSpoken ARRAY_CONTAINS_ANY ('de', 'en');

-- ✗ 20b) JOIN — Firestore has no joins. Should return a clear error,
--       NOT a confusing "Phase 2 arrives" message.
SELECT * FROM "advisors" JOIN "users" ON advisors.id = users.advisor_id;

-- ✗ 20c) GROUP BY — likewise. Clear error.
SELECT * FROM "advisors" GROUP BY gender;

-------------------------------------------------------------------------------
-- A9. Tabularis Table-View wrapper (regression — fixed 2026-05-09)
-------------------------------------------------------------------------------

-- ✓ 21) Table-View synthesises this exact shape with `wrapLimitSubquery: true`.
--      Our parser unwraps it and queries the inner collection.
SELECT * FROM (
  SELECT * FROM "advisors"
  WHERE verified = true
  ORDER BY createdAt DESC
  LIMIT 50
) AS limited_subset;

-------------------------------------------------------------------------------
-- A10. Column projection (client-side — Firestore has no field selection)
-------------------------------------------------------------------------------

-- ✓ 22) Single column
SELECT advisorCode FROM "advisors" LIMIT 5;

-- ✓ 23) Multi-column with WHERE
SELECT id, email, rating FROM "advisors" WHERE verified = true LIMIT 10;

-- ✓ 24) Projection of a column that may be absent from sampled schema
--      → kept in the output as null rather than silently dropped.
SELECT id, rating, totalReviews FROM "advisors" LIMIT 5;

-- ✓ 25) Projection mixed with ORDER BY
SELECT id, firstName, lastName, rating FROM "advisors"
  ORDER BY rating DESC LIMIT 5;

===============================================================================
SECTION B — TABULARIS UI INTERACTIONS (manual, not SQL)
===============================================================================

-- These aren't queries — exercise them by clicking in the Tabularis UI.

-- ☐ B1) Open the connection. Sidebar should list all root collections,
--       sorted alphabetically.
-- ☐ B2) Click `advisors`. Grid loads, ~15 string columns + map columns
--       rendered as JSON-stringified cells.
-- ☐ B3) Hover over a `commission` cell. Tooltip should show the JSON
--       structure as text (no `[object Object]`). If broken: see todo.md
--       about Tabularis upstream issue #24 / DataGrid.tsx:1155.
-- ☐ B4) Open ER diagram (graph icon top-right). Foreign-key lines should
--       connect collections that have `reference` fields between them.
-- ☐ B5) Click "Explain Plan" on a query. `documents_returned`,
--       `documents_scanned`, `index_used` should appear.
-- ☐ B6) Hit a query that needs a composite index. The error block should
--       include a clickable Firebase Console URL.

===============================================================================
SECTION C — PHASE 3: CRUD (write access)
===============================================================================

-- The grid is now editable. To exercise CRUD via the UI:
--
-- ☐ C1) Insert: click "+" in the grid toolbar, fill form, submit.
--      Verify the new row appears AND total_count increments by 1.
--      (If total_count stays the same, the cache invalidation is broken.)
--
-- ☐ C2) Update: double-click any non-id cell, change the value, blur.
--      Verify the change persists across a refresh.
--
-- ☐ C3) Delete: select row(s), Del key (or context menu).
--      Verify total_count decrements.
--
-- ☐ C4) Try editing the synthetic `id` column. Should be rejected with a
--      structured error explaining "delete + re-insert with the new id".
--
-- ☐ C5) Insert without an `id` field. Firestore should auto-generate one;
--      the response should include the new id; the row appears.
--
-- ☐ C6) Edit a `map`-typed cell (e.g. `commission`). The JSON-stringified
--      value should be parseable when re-saved (because we JSON-parse on
--      write — see coercion.rs::coerce_string with hint "map").
--
-- The smoke test below was run end-to-end against luninora-dev on
-- 2026-05-09 via stdin pipe — same operations, no UI:

-- ===== Reproducible CLI smoke test =====
-- Pipe these JSON-RPC frames into ./target/release/firestore-plugin
-- (one per line, in order):
--
--   {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"settings":{"project_id":"luninora-dev","database_id":"(default)","sample_size":50}}}
--   {"jsonrpc":"2.0","id":2,"method":"insert_record","params":{"table":"advisors","data":{"id":"firestore-plugin-smoke-test","email":"smoke@test.local","firstName":"Smoke","lastName":"Test","verified":false,"rating":3.5}}}
--   {"jsonrpc":"2.0","id":3,"method":"execute_query","params":{"query":"SELECT id, email, firstName, verified, rating FROM \"advisors\" WHERE id = 'firestore-plugin-smoke-test'"}}
--   {"jsonrpc":"2.0","id":4,"method":"update_record","params":{"table":"advisors","pkCol":"id","pkVal":"firestore-plugin-smoke-test","colName":"firstName","newVal":"SmokeUpdated"}}
--   {"jsonrpc":"2.0","id":5,"method":"execute_query","params":{"query":"SELECT id, firstName FROM \"advisors\" WHERE id = 'firestore-plugin-smoke-test'"}}
--   {"jsonrpc":"2.0","id":6,"method":"delete_record","params":{"table":"advisors","pkCol":"id","pkVal":"firestore-plugin-smoke-test"}}
--   {"jsonrpc":"2.0","id":7,"method":"execute_query","params":{"query":"SELECT id FROM \"advisors\" WHERE id = 'firestore-plugin-smoke-test'"}}
--
-- Expected: id 7 returns rows: [], total_count: 0.

===============================================================================
SECTION D — TYPE COERCION (Phase 3 — exercise via UI grid)
===============================================================================

-- Each row below is a write that should round-trip through coercion.rs
-- and come back identical when re-read. Use a disposable advisor doc
-- (e.g. `firestore-plugin-coercion-test`) to keep prod data clean.

-- ☐ D1) String → string
--      Edit firstName → "Hello"           expect: "Hello"
-- ☐ D2) Integer → integer  (whole number)
--      Edit experienceYears → 7           expect: 7 (NOT 7.0)
-- ☐ D3) Float → double
--      Edit pricePerMinute → 2.99         expect: 2.99
-- ☐ D4) Boolean
--      Edit verified → true               expect: true
-- ☐ D5) Null
--      Edit description → (clear cell)    expect: null
-- ☐ D6) Timestamp (RFC3339 string)
--      Edit createdAt → 2026-05-09T10:30:00Z  expect: TimestampValue
-- ☐ D7) JSON-stringified map
--      Edit commission → {"a":1}          expect: MapValue {a: 1}
-- ☐ D8) JSON-stringified array
--      Edit labels → ["x","y"]            expect: ArrayValue ["x", "y"]
-- ☐ D9) Malformed JSON in a map column → falls back to string
--      Edit commission → {invalid          expect: StringValue "{invalid"
-- ☐ D10) Reference (full path string)
--      Edit a `reference` column → projects/luninora-dev/databases/(default)/documents/users/X
--      expect: ReferenceValue (link clickable in grid)

===============================================================================
SECTION E — JSON-RPC PROTOCOL CORRECTNESS
===============================================================================

-- Run via raw stdin pipe; verify response counts.

-- ☐ E1) Notification (no `id` field) — MUST receive zero responses.
--      echo '{"jsonrpc":"2.0","method":"ping","params":{}}' | ./firestore-plugin
--
-- ☐ E2) Request with `"id": null` — MUST receive a response with id: null.
--      echo '{"jsonrpc":"2.0","id":null,"method":"ping","params":{}}' | ./firestore-plugin
--
-- ☐ E3) Plain request — receives a response.
--      echo '{"jsonrpc":"2.0","id":42,"method":"ping","params":{}}' | ./firestore-plugin

===============================================================================
SECTION F — ERROR HANDLING
===============================================================================

-- ✗ F1) Unparseable JSON
--      echo 'not json' | ./firestore-plugin
--      Expect: -32700 parse error
--
-- ✗ F2) Unknown method
--      Expect: -32601 method 'foo' is not implemented
--
-- ✗ F3) execute_query before initialize
--      Expect: -32602 plugin not initialised
--
-- ✗ F4) PERMISSION_DENIED — use a SA with no Firestore role
--      Expect: structured error pointing to roles/datastore.viewer
--
-- ✗ F5) update_record on `id` column
--      Expect: -32602 with delete + re-insert guidance
--
-- ✗ F6) delete_record without pk_val
--      Expect: -32602 missing 'pk_val' parameter

-- ✗ F7) DML statement in Console tab — friendly redirect, not parser noise.
INSERT INTO test SET id = 'abc';
--      Expect: -32602 "INSERT via SQL is not supported … use the table tab".
UPDATE test SET test = 'x' WHERE id = 'abc';
--      Expect: same shape, "UPDATE via SQL is not supported …".
DELETE FROM test WHERE id = 'abc';
--      Expect: same shape, "DELETE via SQL is not supported …".

===============================================================================
SECTION G — `test` COLLECTION + SCHEMA OVERRIDES (luninora-dev)
===============================================================================

-- This section validates the schema-overrides feature against the `test`
-- collection on luninora-dev. The override file at
-- ~/firestore-schemas/luninora-dev.json declares:
--
--   { "test":  { "required": true,  "type": "string", "comment": "Hauptfeld" } }
--   { "test2": { "required": false, "type": "string", "comment": "Optionales Zweitfeld" } }
--
-- Prerequisite: in Tabularis Plugin-Settings, set
--   "Schema Overrides Directory" = /home/newt/firestore-schemas
-- then restart Tabularis (not just toggle — plugin process needs respawn).

-------------------------------------------------------------------------------
-- G1. Schema introspection
-------------------------------------------------------------------------------

-- ☐ G1) Open the `test` collection in Tabularis. The column inspector / form
--      should show:
--        id     → required (synthetic), comment "Firestore document ID"
--        test   → required, comment "Hauptfeld der Test-Collection"
--        test2  → optional, comment "Optionales Zweitfeld"
--      If `test` shows up as optional, the override file is not loaded.
--      Check the schema_overrides_dir setting + Tabularis restart.

-- ☐ G2) ER diagram view: `test` should appear with the same column shape.
--       (No FKs because no `reference` fields.)

-------------------------------------------------------------------------------
-- G2. Read-back queries (Phase 2 sanity check on this collection)
-------------------------------------------------------------------------------

-- ✓ G3) Plain SELECT
SELECT * FROM "test" LIMIT 10;

-- ✓ G4) Filter by required field
SELECT * FROM "test" WHERE test = 'placeholder' LIMIT 5;

-- ✓ G5) Filter by doc-id (synthetic id rewrite)
SELECT * FROM "test" WHERE id = 'plugin-required-test';

-- ✓ G6) Projection of just the required field
SELECT id, test FROM "test" LIMIT 5;

-------------------------------------------------------------------------------
-- G3. CRUD via UI — required-field validation
-------------------------------------------------------------------------------

-- ☐ G7) Open the `test` table tab. Click "+" to insert a new row. Fill ONLY
--      the `id` field (e.g. "g7-test"), leave `test` empty, submit.
--      Expect: error in the modal:
--        "Insert failed: Required field(s) not set: test. The plugin's schema
--         declares these as is_nullable=false (likely via your schema-overrides
--         file). Fill them in or mark the field optional in the override."
--      The modal should NOT close. The doc should NOT be created. (Verify
--      with G3 → no g7-test row.)
--
-- ☐ G8) Same form, now fill `test` = "ok", leave test2 empty, submit.
--      Expect: success, modal closes, row appears in grid.
--      total_count for the collection bumps by 1.
--
-- ☐ G9) Insert with all three fields:
--      id="g9-test", test="primary", test2="secondary"
--      Expect: success.
--
-- ☐ G10) Edit the inserted row's `test` field, change to "updated", blur.
--      Expect: persisted; refresh shows "updated".
--
-- ☐ G11) Try to clear the required `test` field (set to empty string). The
--      cell save MIGHT fall through (Tabularis upstream issue: NewRowModal-
--      style required-validation isn't re-enforced on cell-edit). Document
--      what you observe — this is a known limitation.
--
-- ☐ G12) Delete both rows (g8 + g9). total_count back to baseline.

-------------------------------------------------------------------------------
-- G4. Override-file edits (no restart needed for the file itself, but
--     Tabularis caches column metadata until reconnect)
-------------------------------------------------------------------------------

-- ☐ G13) Edit ~/firestore-schemas/luninora-dev.json — flip `test` to
--      "required": false. Save.
--
-- ☐ G14) Toggle the connection in Tabularis (disconnect → reconnect).
--      Open `test` table again.
--      Expect: column inspector now shows `test` as optional.
--
-- ☐ G15) Insert with only `id`. Now SUCCEEDS (no required-field error).
--
-- ☐ G16) Revert the override file back to required. Reconnect. Verify G7
--      again returns the error.

-------------------------------------------------------------------------------
-- G5. Override edge cases
-------------------------------------------------------------------------------

-- ☐ G17) Type override: in the override file, set test2.type = "number".
--      Reconnect. The grid should now treat test2 as numeric (input
--      validation, sort order). Existing string values become "mixed" or
--      coerce on display.
--
-- ☐ G18) Hidden field: add `"hidden": true` to test2 in the override.
--      Reconnect. test2 disappears from the grid entirely.
--
-- ☐ G19) Extra field: declare `extra_fields.notes` with type=string,
--      comment="Freitext". Reconnect. The grid shows a `notes` column even
--      though no doc has that field. Insert a row that fills `notes` —
--      verify it round-trips.

-------------------------------------------------------------------------------
-- G6. CLI smoke for the override pipeline
-------------------------------------------------------------------------------

-- Reproducible from the terminal (bypasses Tabularis entirely):
--
--   {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"settings":{"project_id":"luninora-dev","database_id":"(default)","sample_size":50,"schema_overrides_dir":"/home/newt/firestore-schemas"}}}
--   {"jsonrpc":"2.0","id":2,"method":"get_columns","params":{"table":"test"}}
--      → expect 3 columns: id (is_pk:true), test (is_nullable:false, comment "Hauptfeld..."), test2 (is_nullable:true)
--
--   {"jsonrpc":"2.0","id":3,"method":"insert_record","params":{"table":"test","data":{"id":"cli-only"}}}
--      → expect: {"error":{"code":-32602,"message":"Required field(s) not set: test. ..."}}
--
--   {"jsonrpc":"2.0","id":4,"method":"insert_record","params":{"table":"test","data":{"id":"cli-only","test":"hello"}}}
--      → expect: {"result":1}
--
--   {"jsonrpc":"2.0","id":5,"method":"delete_record","params":{"table":"test","pk_col":"id","pk_val":"cli-only"}}
--      → expect: {"result":1}
