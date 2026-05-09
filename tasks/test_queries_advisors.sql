-- Phase 2 smoke-test corpus against the `advisors` collection on luninora-dev.
-- Schema sampled 2026-05-09 via get_columns. Run these in Tabularis' Console tab
-- (one at a time) and verify the documented expectation.
--
-- Connection: luninora-dev / (default) / Firestore plugin
-- Plugin binary: target/release/firestore-plugin
--
-- Legend
--   ✓ expected to succeed
--   ✗ expected to fail with a specific, structured error (validation/index)
--   ⚠ may fail on missing composite index — that is the test, not a bug

-------------------------------------------------------------------------------
-- 1. Basic SELECT / ORDER BY / LIMIT
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
-- 2. IN / NOT IN
-------------------------------------------------------------------------------

-- ✓ 6) IN list
SELECT * FROM "advisors" WHERE gender IN ('female', 'male', 'diverse') LIMIT 20;

-- ✓ 7) NOT IN list
SELECT * FROM "advisors" WHERE gender NOT IN ('male') LIMIT 20;

-------------------------------------------------------------------------------
-- 3. Document ID (synthetic `id` column → Firestore __name__ rewrite)
-------------------------------------------------------------------------------

-- ✓ 8) Direct doc-id lookup
SELECT * FROM "advisors" WHERE id = 'callservice';

-- ✓ 9) doc-id IN list (multi-key fetch)
SELECT * FROM "advisors" WHERE id IN ('callservice', '5mgfvqFS7QgxJvxAaqViiruxqy13');

-- ✓ 10) Plain string equality (verifies we still hit non-id strings)
SELECT * FROM "advisors" WHERE email = 'jan.poblenz+advisorverification2@luninora.de';

-------------------------------------------------------------------------------
-- 4. Array operators (Firestore-specific)
-------------------------------------------------------------------------------

-- ✓ 11) ARRAY_CONTAINS — single-value membership
SELECT * FROM "advisors" WHERE products ARRAY_CONTAINS 'call' LIMIT 20;

-- ✓ 12) ARRAY_CONTAINS_ANY — multi-value membership
SELECT * FROM "advisors" WHERE languagesSpoken ARRAY_CONTAINS_ANY ('de', 'en') LIMIT 20;

-------------------------------------------------------------------------------
-- 5. Timestamp literals
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
-- 6. OR / parens (Boolean tree)
-------------------------------------------------------------------------------

-- ✓ 15) OR — Firestore Filter.or under the hood
SELECT * FROM "advisors" WHERE verified = true OR emailVerified = true LIMIT 20;

-- ✓ 16) Parens override precedence
SELECT * FROM "advisors"
  WHERE (verified = true OR isListedByAdmin = true)
    AND productsEnabled = true
  LIMIT 20;

-------------------------------------------------------------------------------
-- 7. Pagination
-------------------------------------------------------------------------------

-- ✓ 17) OFFSET-based pagination
--      Page 1 takes the OFFSET path; Page 2+ should hit CURSOR_CACHE and
--      switch to start_after(). Watch stderr to confirm.
SELECT * FROM "advisors" ORDER BY createdAt DESC LIMIT 5 OFFSET 10;

-------------------------------------------------------------------------------
-- 8. Failure modes — these MUST return a structured error
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

-------------------------------------------------------------------------------
-- 9. Tabularis Table-View wrapper (regression — see #17 fix on 2026-05-09)
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
-- 10. Column projection (client-side — Firestore has no field selection)
-------------------------------------------------------------------------------

-- ✓ 22) Single column
SELECT advisorCode FROM "advisors" LIMIT 5;

-- ✓ 23) Multi-column with WHERE
SELECT id, email, rating FROM "advisors" WHERE verified = true LIMIT 10;

-- ✓ 24) Projection of a column that may be absent from sampled schema
--      → kept in the output as null rather than silently dropped.
SELECT id, rating, totalReviews FROM "advisors" LIMIT 5;
