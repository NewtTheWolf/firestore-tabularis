# Phase 2 — Firestore Query Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Phase-1 read-only viewer into a daily-driver query tool — full Firestore filter set with AND/OR/NOT/parens, native nested JSON in row payloads, real `total_count` via cached parallel COUNT aggregations, cursor-based pagination for sequential pages, an ER-diagram via inferred Reference foreign keys, and four new structured error mappings (PERMISSION_DENIED, RESOURCE_EXHAUSTED, DEADLINE_EXCEEDED, UNAVAILABLE).

**Architecture:** Existing async dispatch loop and lazy `FirestoreDb` stay untouched. Two new pure-logic modules: `cache` (generic TTL+LRU used for cursor and count caches) and `firestore_filter` (FilterExpr AST → firestore-rs Filter mapper plus pre-flight Firestore-restriction validation). The Phase-1 query parser is rewritten with a boolean-tree AST. `execute_query` evolves in three steps: filter integration → parallel COUNT cache → cursor pagination. `schema_infer` gains reference-target extraction for the ER diagram.

**Tech Stack:** Rust stable, existing dependency tree (no new crates — we hand-roll the TTL+LRU cache for ~80 lines vs pulling `lru`).

**Spec:** [`docs/superpowers/specs/2026-05-08-phase-2-firestore-query-layer-design.md`](../specs/2026-05-08-phase-2-firestore-query-layer-design.md)

---

## File map

| Path | Disposition | Responsibility |
|---|---|---|
| `src/cache.rs` | **create** | Generic `TtlLruCache<K, V>` |
| `src/query_parser.rs` | rewrite | Boolean-tree AST, new tokens, WHERE/AND/OR/NOT/IN/ARRAY_CONTAINS/parens grammar |
| `src/firestore_filter.rs` | **create** | `FilterExpr` validation + firestore-rs Filter builder |
| `src/state.rs` | modify | `CURSOR_CACHE` + `COUNT_CACHE` globals |
| `src/handlers/query.rs` | modify | `execute_query` with filter + COUNT + cursor; `explain_query` real |
| `src/handlers/metadata.rs` | modify | `get_schema_snapshot` populated with FKs |
| `src/schema_infer.rs` | modify | `references: Option<String>` field; native JSON for Map/Array; reference-target extraction |
| `src/firestore_error.rs` | modify | 4 new `ErrorKind` variants + project-id substitution |
| `src/main.rs` | modify | Add `mod cache; mod firestore_filter;` |
| `tests/firestore_emulator.rs` | modify | Phase-2 integration test block |
| `tests/fixtures/seed.sh` | **create** | Seed script for emulator fixtures |
| `CLAUDE.md` | modify | Final pass after implementation |
| `docs/ROADMAP.md` | modify | Mark Phase 2 shipped |

---

## Conventions

- All `cargo` commands run from `/home/newt/Projekte/Personal/NewtTheWolf/firestore-driver`.
- Stage exact file paths in `git add` rather than `git add -A`.
- After every task: `cargo build && cargo test` must pass.
- Don't add features the spec doesn't ask for. If a step seems to need more code than shown, re-read the spec section.
- For tasks that touch firestore-rs APIs (Tasks 5, 8, 9, 12, 13), the exact API names may have drifted from this plan — check rustdoc and adapt. The contract is what matters; the method names are an implementation detail.

---

## Task 1: TTL+LRU cache

**Files:**
- Create: `src/cache.rs`
- Modify: `src/main.rs`

Pure-logic module — no Firestore deps, ideal for tight TDD.

- [ ] **Step 1: Create the module skeleton**

Create `src/cache.rs`:

```rust
//! Bounded cache with TTL eviction.
//!
//! Used for CURSOR_CACHE and COUNT_CACHE in `state.rs`. We hand-roll this rather
//! than pulling the `lru` crate — ~80 lines of code, zero dependencies, and the
//! contract is narrow enough that maintenance is trivial.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

pub struct TtlLruCache<K: Hash + Eq + Clone, V> {
    capacity: usize,
    ttl: Duration,
    entries: HashMap<K, Entry<V>>,
    /// Insertion order for LRU eviction. Front = oldest.
    order: std::collections::VecDeque<K>,
}

struct Entry<V> {
    value: V,
    inserted_at: Instant,
}

impl<K: Hash + Eq + Clone, V> TtlLruCache<K, V> {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            capacity,
            ttl,
            entries: HashMap::with_capacity(capacity),
            order: std::collections::VecDeque::with_capacity(capacity),
        }
    }

    pub fn get(&mut self, _key: &K) -> Option<&V> {
        None
    }

    pub fn insert(&mut self, _key: K, _value: V) {}

    pub fn remove(&mut self, _key: &K) {}

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

#[cfg(test)]
mod tests {}
```

In `src/main.rs`, add `mod cache;` next to the other `mod` declarations (between `mod client;` and `mod error;` to keep alphabetical order).

Run: `cargo build`
Expected: builds with dead-code warnings.

- [ ] **Step 2: First failing test — empty cache misses**

Replace `mod tests {}` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_miss_on_empty_cache() {
        let mut c: TtlLruCache<String, u64> = TtlLruCache::new(10, Duration::from_secs(60));
        assert!(c.get(&"x".to_string()).is_none());
    }
}
```

Run: `cargo test cache::tests::lookup_miss_on_empty_cache`
Expected: PASS (the stub already returns None).

- [ ] **Step 3: Failing test — inserted value is retrievable**

Append:

```rust
    #[test]
    fn inserted_value_is_retrievable() {
        let mut c: TtlLruCache<String, u64> = TtlLruCache::new(10, Duration::from_secs(60));
        c.insert("x".to_string(), 42);
        assert_eq!(c.get(&"x".to_string()), Some(&42));
        assert_eq!(c.len(), 1);
    }
```

Run: `cargo test cache::tests::inserted_value_is_retrievable`
Expected: FAIL — get still returns None.

- [ ] **Step 4: Implement get/insert minimally**

Replace the bodies of `get` and `insert`:

```rust
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let entry = self.entries.get(key)?;
        if entry.inserted_at.elapsed() > self.ttl {
            self.entries.remove(key);
            self.order.retain(|k| k != key);
            return None;
        }
        Some(&entry.value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        // If key already present, refresh in place (don't grow order).
        if self.entries.contains_key(&key) {
            self.entries.insert(
                key.clone(),
                Entry { value, inserted_at: Instant::now() },
            );
            return;
        }
        // Evict oldest while at capacity.
        while self.entries.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, Entry { value, inserted_at: Instant::now() });
    }

    pub fn remove(&mut self, key: &K) {
        self.entries.remove(key);
        self.order.retain(|k| k != key);
    }
```

Run: `cargo test cache::tests`
Expected: 2 passing.

- [ ] **Step 5: Coverage tests for TTL and LRU**

Append:

```rust
    #[test]
    fn expired_entry_returns_miss() {
        let mut c: TtlLruCache<String, u64> = TtlLruCache::new(10, Duration::from_millis(10));
        c.insert("x".to_string(), 1);
        std::thread::sleep(Duration::from_millis(20));
        assert!(c.get(&"x".to_string()).is_none());
    }

    #[test]
    fn lru_eviction_on_capacity() {
        let mut c: TtlLruCache<u64, &str> = TtlLruCache::new(2, Duration::from_secs(60));
        c.insert(1, "a");
        c.insert(2, "b");
        c.insert(3, "c"); // evicts key 1
        assert!(c.get(&1).is_none());
        assert_eq!(c.get(&2), Some(&"b"));
        assert_eq!(c.get(&3), Some(&"c"));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn reinsert_refreshes_in_place() {
        let mut c: TtlLruCache<u64, u64> = TtlLruCache::new(2, Duration::from_secs(60));
        c.insert(1, 100);
        c.insert(1, 200);
        assert_eq!(c.get(&1), Some(&200));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn explicit_remove_clears_entry() {
        let mut c: TtlLruCache<u64, u64> = TtlLruCache::new(10, Duration::from_secs(60));
        c.insert(1, 100);
        c.remove(&1);
        assert!(c.get(&1).is_none());
    }

    #[test]
    fn clear_drops_all_entries() {
        let mut c: TtlLruCache<u64, u64> = TtlLruCache::new(10, Duration::from_secs(60));
        c.insert(1, 100);
        c.insert(2, 200);
        c.clear();
        assert_eq!(c.len(), 0);
    }
```

Run: `cargo test cache::tests`
Expected: 6 passing.

- [ ] **Step 6: Commit**

```bash
git add src/cache.rs src/main.rs
git commit -m "feat(cache): TTL+LRU cache for cursor and count caches"
```

---

## Task 2: Query parser — new AST shape and tokens

**Files:**
- Modify: `src/query_parser.rs`

This task replaces the Phase-1 `ParsedQuery` struct (which had `table`, `order_by`, `limit`, `offset`) with the boolean-tree shape from the spec. Tokenizer learns multi-char ops, string literals, and parens. WHERE parsing comes in Task 3 — this task only sets up types and lexer.

- [ ] **Step 1: Replace the AST types**

In `src/query_parser.rs`, replace the existing `ParsedQuery` and `OrderItem` definitions with:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuery {
    pub table: String,
    pub where_clause: Option<FilterExpr>,
    pub order_by: Vec<OrderItem>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderItem {
    pub field: String,
    pub desc: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpr {
    Compare {
        field: Vec<String>,
        op: CmpOp,
        value: Literal,
    },
    In {
        field: Vec<String>,
        values: Vec<Literal>,
        negated: bool,
    },
    ArrayContains {
        field: Vec<String>,
        value: Literal,
    },
    ArrayContainsAny {
        field: Vec<String>,
        values: Vec<Literal>,
    },
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp { Eq, Ne, Lt, Le, Gt, Ge }

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    Timestamp(chrono::DateTime<chrono::Utc>),
}
```

The Phase-1 tests assume `where_clause` is absent on `SELECT * FROM "users"` — keep them passing by defaulting to `None` in the parser. We'll wire `where_clause = None` for the existing happy paths in this task and add WHERE parsing in Task 3.

- [ ] **Step 2: Replace the Token enum and tokenizer**

Replace the Phase-1 `Token` enum and `tokenize` function with:

```rust
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Star,
    Comma,
    Number(u64),
    /// String literal in single quotes, with `\'` and `''` accepted as escaped quote.
    StringLit(String),
    LParen,
    RParen,
    /// Negative integer or float literal (the leading `-` was consumed during number parsing).
    NegNumber(u64),
    Op(CmpOp),
    /// Numeric literal that includes a fractional part.
    Float(f64),
    /// Catch-all for unrecognised punctuation. The parser surfaces a clear error.
    Symbol(char),
}

fn tokenize(sql: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '*' { out.push(Token::Star); i += 1; continue; }
        if c == ',' { out.push(Token::Comma); i += 1; continue; }
        if c == '(' { out.push(Token::LParen); i += 1; continue; }
        if c == ')' { out.push(Token::RParen); i += 1; continue; }

        // Multi-char operators first.
        if c == '=' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                out.push(Token::Op(CmpOp::Eq));
                i += 2;
            } else {
                out.push(Token::Op(CmpOp::Eq));
                i += 1;
            }
            continue;
        }
        if c == '<' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                out.push(Token::Op(CmpOp::Le));
                i += 2;
            } else if i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                out.push(Token::Op(CmpOp::Ne));
                i += 2;
            } else {
                out.push(Token::Op(CmpOp::Lt));
                i += 1;
            }
            continue;
        }
        if c == '>' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                out.push(Token::Op(CmpOp::Ge));
                i += 2;
            } else {
                out.push(Token::Op(CmpOp::Gt));
                i += 1;
            }
            continue;
        }
        if c == '!' && i + 1 < bytes.len() && bytes[i + 1] == b'=' {
            out.push(Token::Op(CmpOp::Ne));
            i += 2;
            continue;
        }

        // Quoted identifiers: " or `
        if c == '"' || c == '`' {
            let quote = c;
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] as char != quote {
                i += 1;
            }
            if i >= bytes.len() {
                return Err(format!("unterminated identifier starting with {quote}"));
            }
            let ident = std::str::from_utf8(&bytes[start..i])
                .map_err(|e| e.to_string())?
                .to_string();
            i += 1;
            out.push(Token::Word(ident));
            continue;
        }

        // String literal: '...'
        if c == '\'' {
            let start = i + 1;
            let mut buf = String::new();
            i += 1;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch == '\\' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    buf.push('\'');
                    i += 2;
                    continue;
                }
                if ch == '\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        buf.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                buf.push(ch);
                i += 1;
            }
            if i >= bytes.len() {
                return Err(format!("unterminated string literal starting at {start}"));
            }
            i += 1; // skip closing quote
            out.push(Token::StringLit(buf));
            continue;
        }

        // Number literal (integer or float).
        if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] as char == '.' {
                i += 1;
                while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                    i += 1;
                }
                let f: f64 = std::str::from_utf8(&bytes[start..i])
                    .map_err(|e| e.to_string())?
                    .parse()
                    .map_err(|e: std::num::ParseFloatError| e.to_string())?;
                out.push(Token::Float(f));
            } else {
                let n: u64 = std::str::from_utf8(&bytes[start..i])
                    .map_err(|e| e.to_string())?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
                out.push(Token::Number(n));
            }
            continue;
        }

        // Identifier / keyword (also: dot-notation field paths consume `.` as part of the word).
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
                    i += 1;
                } else {
                    break;
                }
            }
            let ident = std::str::from_utf8(&bytes[start..i])
                .map_err(|e| e.to_string())?
                .to_string();
            out.push(Token::Word(ident));
            continue;
        }

        return Err(format!("unexpected character: {c}"));
    }
    Ok(out)
}
```

Note the dot is now consumed as part of identifier characters so `address.city` lexes as a single `Word("address.city")` — the parser splits on `.` to produce `Vec<String>` field paths.

The `NegNumber` variant declared above isn't produced by this tokenizer — leading `-` is still rejected at lex time, leaving negative-literal handling for a future phase. The variant is reserved for forward compatibility; remove it if clippy complains.

- [ ] **Step 3: Update parse() to default where_clause to None**

The Phase-1 `parse_select` produces the full ParsedQuery. Update its end where it returns `Ok(ParsedQuery { ... })`:

```rust
        Ok(ParsedQuery {
            table,
            where_clause: None, // populated in Task 3
            order_by,
            limit,
            offset,
        })
```

Also add WHERE detection so Task-3 grammar isn't blocked: in the keyword-loop in `parse_select`, add a branch:

```rust
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("WHERE") => {
                    return Err("WHERE arrives in Task 3 of Phase 2 — placeholder error".into());
                }
```

This intentional placeholder error gets removed in Task 3 when we wire WHERE parsing. For now it ensures Phase-1 tests that test for "Phase 2" messages still match since WHERE was previously a Phase-2 reject.

Wait — Phase 1's `rejects_where_clause` test asserts `err.contains("Phase 2") && err.contains("WHERE")`. The placeholder above doesn't contain "WHERE". Update to:

```rust
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("WHERE") => {
                    return Err("WHERE arrives in Task 3 of Phase 2 — placeholder; existing test assertion: Phase 2".into());
                }
```

- [ ] **Step 4: Verify Phase-1 tests still pass**

```bash
cargo test query_parser::tests
```
Expected: all 13 Phase-1 tests still pass.

If `rejects_where_clause` fails because the placeholder error message changed, adjust either the message to include the literal `"WHERE"` and `"Phase 2"` substrings, or update the test expectation in the next step.

- [ ] **Step 5: Add Float-literal-tokenization tests**

These don't depend on WHERE parsing — they exercise the new tokenizer in isolation. Append to the `mod tests` block:

```rust
    #[test]
    fn float_literals_lex_correctly() {
        // We can't directly call the private tokenize() from outside, but we can
        // exercise it by feeding a query that would lex a Float and observing the
        // parse error message about expected position.
        let err = parse("SELECT * FROM \"x\" 3.14").unwrap_err();
        assert!(err.contains("trailing tokens") || err.contains("unexpected"));
    }

    #[test]
    fn dot_notation_in_table_name_is_preserved() {
        // The current parser will accept dot-notation as a single Word token.
        let q = parse("SELECT * FROM users.archive").unwrap();
        assert_eq!(q.table, "users.archive");
    }
```

Run: `cargo test query_parser::tests`
Expected: 15 passing (13 Phase-1 + 2 new).

- [ ] **Step 6: Commit**

```bash
git add src/query_parser.rs
git commit -m "feat(query_parser): boolean-tree AST, multi-char ops, string literals, parens"
```

---

## Task 3: Query parser — WHERE clause grammar

**Files:**
- Modify: `src/query_parser.rs`

Implements the recursive-descent `parse_or` → `parse_and` → `parse_not` → `parse_atom` chain. Atoms are comparisons, IN/NOT IN, ARRAY_CONTAINS, ARRAY_CONTAINS_ANY, or a parenthesised `parse_or`.

- [ ] **Step 1: Add a failing test for the simplest WHERE clause**

In `src/query_parser.rs` `mod tests`, append:

```rust
    #[test]
    fn parses_where_with_eq() {
        let q = parse(r#"SELECT * FROM "u" WHERE name = 'Alice'"#).unwrap();
        let w = q.where_clause.unwrap();
        match w {
            FilterExpr::Compare { field, op, value } => {
                assert_eq!(field, vec!["name".to_string()]);
                assert_eq!(op, CmpOp::Eq);
                assert_eq!(value, Literal::Str("Alice".to_string()));
            }
            _ => panic!("expected Compare, got {w:?}"),
        }
    }
```

Run: `cargo test query_parser::tests::parses_where_with_eq`
Expected: FAIL with the placeholder "WHERE arrives in Task 3" message.

- [ ] **Step 2: Replace placeholder with full WHERE parsing**

Find the placeholder branch in `parse_select`:
```rust
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("WHERE") => {
                    return Err("WHERE arrives in Task 3 of Phase 2 ...".into());
                }
```

Replace with:
```rust
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("WHERE") => {
                    self.advance();
                    let expr = self.parse_or()?;
                    where_clause = Some(expr);
                }
```

And declare `where_clause` at the top of `parse_select`'s body (next to `order_by`, `limit`, `offset`):
```rust
        let mut where_clause: Option<FilterExpr> = None;
```

Then thread `where_clause` into the final `Ok(ParsedQuery { ... })`.

Add the parser methods inside `impl Parser`:

```rust
    fn parse_or(&mut self) -> Result<FilterExpr, String> {
        let mut terms = vec![self.parse_and()?];
        while let Some(Token::Word(w)) = self.peek() {
            if !w.eq_ignore_ascii_case("OR") {
                break;
            }
            self.advance();
            terms.push(self.parse_and()?);
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap() } else { FilterExpr::Or(terms) })
    }

    fn parse_and(&mut self) -> Result<FilterExpr, String> {
        let mut terms = vec![self.parse_not()?];
        while let Some(Token::Word(w)) = self.peek() {
            if !w.eq_ignore_ascii_case("AND") {
                break;
            }
            self.advance();
            terms.push(self.parse_not()?);
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap() } else { FilterExpr::And(terms) })
    }

    fn parse_not(&mut self) -> Result<FilterExpr, String> {
        // NOT prefix is reserved syntax but Phase 2 only uses it in `NOT IN`. A bare `NOT expr`
        // here is Phase-2-rejected to keep the grammar narrow.
        if let Some(Token::Word(w)) = self.peek() {
            if w.eq_ignore_ascii_case("NOT") {
                // Look one ahead: if next is IN, it's a NOT IN, fall through to atom parsing
                // which handles it. Otherwise reject.
                let saved = self.pos;
                self.advance();
                if let Some(Token::Word(w2)) = self.peek() {
                    if w2.eq_ignore_ascii_case("IN") {
                        self.pos = saved;
                        return self.parse_atom();
                    }
                }
                return Err("Phase 2 supports NOT only in NOT IN; bare NOT expressions are not parseable yet".into());
            }
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<FilterExpr, String> {
        // Parenthesised group
        if matches!(self.peek(), Some(Token::LParen)) {
            self.advance();
            let inner = self.parse_or()?;
            match self.advance() {
                Some(Token::RParen) => return Ok(inner),
                other => return Err(format!("expected ')', got {other:?}")),
            }
        }

        // Function call: ARRAY_CONTAINS(...) or ARRAY_CONTAINS_ANY(...)
        if let Some(Token::Word(w)) = self.peek().cloned() {
            if w.eq_ignore_ascii_case("ARRAY_CONTAINS") {
                self.advance();
                return self.parse_array_contains_call(false);
            }
            if w.eq_ignore_ascii_case("ARRAY_CONTAINS_ANY") {
                self.advance();
                return self.parse_array_contains_call(true);
            }
            // Unknown function — only flag when it's followed by `(`, otherwise treat as
            // a field name in a comparison.
            if let Some(Token::LParen) = self.tokens.get(self.pos + 1) {
                let upper = w.to_uppercase();
                return Err(format!(
                    "unknown function '{upper}' (did you mean ARRAY_CONTAINS or ARRAY_CONTAINS_ANY?)"
                ));
            }
        }

        // Field … op … value, or field [NOT] IN (...)
        let field = match self.advance() {
            Some(Token::Word(w)) => w.split('.').map(str::to_string).collect::<Vec<_>>(),
            other => return Err(format!("expected field name in WHERE, got {other:?}")),
        };

        // [NOT] IN
        if let Some(Token::Word(w)) = self.peek() {
            let upper = w.to_uppercase();
            if upper == "NOT" {
                self.advance();
                match self.advance() {
                    Some(Token::Word(w2)) if w2.eq_ignore_ascii_case("IN") => {}
                    other => return Err(format!("expected 'IN' after 'NOT', got {other:?}")),
                }
                let values = self.parse_value_list()?;
                return Ok(FilterExpr::In { field, values, negated: true });
            }
            if upper == "IN" {
                self.advance();
                let values = self.parse_value_list()?;
                return Ok(FilterExpr::In { field, values, negated: false });
            }
        }

        // Comparison
        let op = match self.advance() {
            Some(Token::Op(op)) => op,
            other => return Err(format!("expected comparison operator, got {other:?}")),
        };
        let value = self.parse_literal()?;
        Ok(FilterExpr::Compare { field, op, value })
    }

    fn parse_array_contains_call(&mut self, any: bool) -> Result<FilterExpr, String> {
        match self.advance() {
            Some(Token::LParen) => {}
            other => return Err(format!("expected '(' after ARRAY_CONTAINS{}, got {other:?}",
                                        if any { "_ANY" } else { "" })),
        }
        let field = match self.advance() {
            Some(Token::Word(w)) => w.split('.').map(str::to_string).collect::<Vec<_>>(),
            other => return Err(format!("expected field name, got {other:?}")),
        };
        match self.advance() {
            Some(Token::Comma) => {}
            other => return Err(format!("expected ',' after field name, got {other:?}")),
        }
        let result = if any {
            let values = self.parse_value_list()?;
            FilterExpr::ArrayContainsAny { field, values }
        } else {
            let value = self.parse_literal()?;
            FilterExpr::ArrayContains { field, value }
        };
        match self.advance() {
            Some(Token::RParen) => Ok(result),
            other => Err(format!("expected ')' to close function call, got {other:?}")),
        }
    }

    fn parse_value_list(&mut self) -> Result<Vec<Literal>, String> {
        match self.advance() {
            Some(Token::LParen) => {}
            other => return Err(format!("expected '(' to start value list, got {other:?}")),
        }
        let mut out = Vec::new();
        loop {
            if let Some(Token::RParen) = self.peek() {
                self.advance();
                break;
            }
            out.push(self.parse_literal()?);
            match self.peek() {
                Some(Token::Comma) => { self.advance(); continue; }
                Some(Token::RParen) => { self.advance(); break; }
                other => return Err(format!("expected ',' or ')' in value list, got {other:?}")),
            }
        }
        if out.is_empty() {
            return Err("IN/NOT IN/ARRAY_CONTAINS_ANY requires at least one value".into());
        }
        Ok(out)
    }

    fn parse_literal(&mut self) -> Result<Literal, String> {
        match self.advance() {
            Some(Token::StringLit(s)) => Ok(Literal::Str(s)),
            Some(Token::Number(n)) => Ok(Literal::Int(n as i64)),
            Some(Token::Float(f)) => Ok(Literal::Float(f)),
            Some(Token::Word(w)) => {
                let upper = w.to_uppercase();
                match upper.as_str() {
                    "TRUE" => Ok(Literal::Bool(true)),
                    "FALSE" => Ok(Literal::Bool(false)),
                    "NULL" => Ok(Literal::Null),
                    "TIMESTAMP" => {
                        // Expect StringLit after TIMESTAMP
                        match self.advance() {
                            Some(Token::StringLit(s)) => {
                                let dt = chrono::DateTime::parse_from_rfc3339(&s)
                                    .map_err(|e| format!("TIMESTAMP literal not RFC 3339: {e}"))?
                                    .with_timezone(&chrono::Utc);
                                Ok(Literal::Timestamp(dt))
                            }
                            other => Err(format!("expected string after TIMESTAMP, got {other:?}")),
                        }
                    }
                    _ => Err(format!("expected literal, got identifier '{w}'")),
                }
            }
            other => Err(format!("expected literal, got {other:?}")),
        }
    }
```

- [ ] **Step 3: Run the WHERE happy-path test**

Run: `cargo test query_parser::tests::parses_where_with_eq`
Expected: PASS.

- [ ] **Step 4: Add full coverage tests**

Append to `mod tests`:

```rust
    fn first_compare(q: &ParsedQuery) -> &FilterExpr {
        q.where_clause.as_ref().unwrap()
    }

    #[test]
    fn parses_where_with_double_eq() {
        let q = parse(r#"SELECT * FROM "u" WHERE name == 'Alice'"#).unwrap();
        match first_compare(&q) {
            FilterExpr::Compare { op, .. } => assert_eq!(*op, CmpOp::Eq),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_where_with_ne_and_diamond() {
        let q1 = parse(r#"SELECT * FROM "u" WHERE level != 'debug'"#).unwrap();
        let q2 = parse(r#"SELECT * FROM "u" WHERE level <> 'debug'"#).unwrap();
        for q in &[q1, q2] {
            match first_compare(q) {
                FilterExpr::Compare { op, .. } => assert_eq!(*op, CmpOp::Ne),
                _ => panic!(),
            }
        }
    }

    #[test]
    fn parses_string_literal_with_escaped_quote() {
        let q = parse(r#"SELECT * FROM "u" WHERE name = 'Alice O\'Brien'"#).unwrap();
        match first_compare(&q) {
            FilterExpr::Compare { value: Literal::Str(s), .. } => {
                assert_eq!(s, "Alice O'Brien");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_int_and_float_and_bool_and_null() {
        let q = parse(r#"SELECT * FROM "u" WHERE x = 42 AND y = 3.14 AND z = TRUE AND w = NULL"#).unwrap();
        match q.where_clause {
            Some(FilterExpr::And(ref terms)) => {
                let lits: Vec<&Literal> = terms.iter().map(|t| match t {
                    FilterExpr::Compare { value, .. } => value,
                    _ => panic!(),
                }).collect();
                assert_eq!(lits[0], &Literal::Int(42));
                assert_eq!(lits[1], &Literal::Float(3.14));
                assert_eq!(lits[2], &Literal::Bool(true));
                assert_eq!(lits[3], &Literal::Null);
            }
            _ => panic!("{:?}", q.where_clause),
        }
    }

    #[test]
    fn parses_dot_notation_field_path() {
        let q = parse(r#"SELECT * FROM "u" WHERE address.city = 'Berlin'"#).unwrap();
        match first_compare(&q) {
            FilterExpr::Compare { field, .. } => {
                assert_eq!(field, &vec!["address".to_string(), "city".to_string()]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_in_and_not_in_lists() {
        let q1 = parse(r#"SELECT * FROM "u" WHERE status IN ('active', 'pending')"#).unwrap();
        let q2 = parse(r#"SELECT * FROM "u" WHERE status NOT IN ('banned')"#).unwrap();
        match q1.where_clause {
            Some(FilterExpr::In { values, negated: false, .. }) => {
                assert_eq!(values.len(), 2);
            }
            _ => panic!(),
        }
        match q2.where_clause {
            Some(FilterExpr::In { values, negated: true, .. }) => {
                assert_eq!(values, vec![Literal::Str("banned".to_string())]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_array_contains_and_array_contains_any() {
        let q1 = parse(r#"SELECT * FROM "p" WHERE ARRAY_CONTAINS(tags, 'urgent')"#).unwrap();
        match q1.where_clause {
            Some(FilterExpr::ArrayContains { field, value }) => {
                assert_eq!(field, vec!["tags".to_string()]);
                assert_eq!(value, Literal::Str("urgent".to_string()));
            }
            _ => panic!(),
        }
        let q2 = parse(r#"SELECT * FROM "p" WHERE ARRAY_CONTAINS_ANY(tags, ('p0', 'p1'))"#).unwrap();
        match q2.where_clause {
            Some(FilterExpr::ArrayContainsAny { values, .. }) => {
                assert_eq!(values.len(), 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn precedence_or_under_and() {
        let q = parse(r#"SELECT * FROM "u" WHERE a = 1 OR b = 2 AND c = 3"#).unwrap();
        match q.where_clause {
            Some(FilterExpr::Or(ref terms)) => {
                assert_eq!(terms.len(), 2);
                assert!(matches!(&terms[0], FilterExpr::Compare { .. }));
                match &terms[1] {
                    FilterExpr::And(inner) => assert_eq!(inner.len(), 2),
                    _ => panic!("expected And inside Or"),
                }
            }
            _ => panic!("{:?}", q.where_clause),
        }
    }

    #[test]
    fn parens_override_precedence() {
        let q = parse(r#"SELECT * FROM "u" WHERE (a = 1 OR b = 2) AND c = 3"#).unwrap();
        match q.where_clause {
            Some(FilterExpr::And(ref terms)) => {
                assert_eq!(terms.len(), 2);
                assert!(matches!(&terms[0], FilterExpr::Or(_)));
            }
            _ => panic!("{:?}", q.where_clause),
        }
    }

    #[test]
    fn parses_timestamp_cast() {
        let q = parse(r#"SELECT * FROM "e" WHERE ts > TIMESTAMP '2026-01-01T00:00:00Z'"#).unwrap();
        match first_compare(&q) {
            FilterExpr::Compare { value: Literal::Timestamp(_), .. } => {}
            _ => panic!("{:?}", q.where_clause),
        }
    }

    #[test]
    fn rejects_empty_in_list() {
        let err = parse(r#"SELECT * FROM "u" WHERE x IN ()"#).unwrap_err();
        assert!(err.contains("at least one value"), "got: {err}");
    }

    #[test]
    fn rejects_unbalanced_parens() {
        let err = parse(r#"SELECT * FROM "u" WHERE (a = 1"#).unwrap_err();
        assert!(err.contains(")") || err.contains("expected"));
    }

    #[test]
    fn rejects_unknown_function() {
        let err = parse(r#"SELECT * FROM "u" WHERE FOO_BAR(x)"#).unwrap_err();
        assert!(err.contains("unknown function"), "got: {err}");
        assert!(err.contains("FOO_BAR"));
    }
```

Run: `cargo test query_parser::tests`
Expected: 13 + 2 + 13 = 28 passing.

- [ ] **Step 5: Update Phase-1 `rejects_where_clause` test**

The Phase-1 test asserts WHERE produces a "Phase 2" error. Now WHERE works, so the test is obsolete. Remove it from `mod tests`:

Find:
```rust
    #[test]
    fn rejects_where_clause() {
        let err = parse(r#"SELECT * FROM "users" WHERE id = 1"#).unwrap_err();
        assert!(err.contains("Phase 2"), "expected Phase 2 message, got: {err}");
        assert!(err.contains("WHERE"));
    }
```

Delete it. Run `cargo test query_parser::tests` again — should be 27 passing now (one less).

- [ ] **Step 6: Commit**

```bash
git add src/query_parser.rs
git commit -m "feat(query_parser): WHERE/AND/OR/NOT IN/ARRAY_CONTAINS grammar with boolean tree"
```

---

## Task 4: Firestore filter — pre-flight validation

**Files:**
- Create: `src/firestore_filter.rs`
- Modify: `src/main.rs`

Pure-logic validation that catches Firestore's compound-filter restrictions before they hit the wire. Builder for the actual Firestore filter object lives in Task 5.

- [ ] **Step 1: Create the module skeleton**

Create `src/firestore_filter.rs`:

```rust
//! `FilterExpr` validation against Firestore's compound-filter restrictions,
//! and the builder that maps `FilterExpr` → `firestore::FirestoreQueryFilter`.
//!
//! Validation runs before any Firestore call so the user gets a clear error
//! ("inequality on at most one field per query") instead of Firestore's
//! cryptic gRPC message.

use crate::query_parser::{CmpOp, FilterExpr, Literal};
use std::collections::BTreeSet;

pub fn validate(expr: &FilterExpr) -> Result<(), String> {
    let mut state = ValidationState::default();
    walk(expr, &mut state);

    if state.inequality_fields.len() > 1 {
        let mut fields: Vec<String> = state.inequality_fields.iter().cloned().collect();
        fields.sort();
        return Err(format!(
            "Firestore allows inequality on at most one field per query (saw {}). \
             Adjust the filter or split into multiple queries.",
            fields.join(", ")
        ));
    }

    for (n, kind) in [
        (state.in_max_size, "IN/NOT IN"),
        (state.array_contains_any_max_size, "ARRAY_CONTAINS_ANY"),
    ] {
        if n > 30 {
            return Err(format!(
                "Firestore limits {kind} to 30 values per query (saw {n})."
            ));
        }
    }

    if state.has_array_contains && state.has_array_contains_any {
        return Err(
            "Firestore disallows ARRAY_CONTAINS and ARRAY_CONTAINS_ANY in the same query."
                .into()
        );
    }

    if state.array_contains_fields.values().any(|n| *n > 1) {
        let mut fields: Vec<String> = state
            .array_contains_fields
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|(f, _)| f.clone())
            .collect();
        fields.sort();
        return Err(format!(
            "Firestore allows at most one ARRAY_CONTAINS per field (saw multiple on: {}).",
            fields.join(", ")
        ));
    }

    Ok(())
}

#[derive(Default)]
struct ValidationState {
    inequality_fields: BTreeSet<String>,
    in_max_size: usize,
    array_contains_any_max_size: usize,
    has_array_contains: bool,
    has_array_contains_any: bool,
    array_contains_fields: std::collections::BTreeMap<String, usize>,
}

fn walk(expr: &FilterExpr, state: &mut ValidationState) {
    match expr {
        FilterExpr::Compare { field, op, .. } => {
            if matches!(op, CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge | CmpOp::Ne) {
                state.inequality_fields.insert(field.join("."));
            }
        }
        FilterExpr::In { values, .. } => {
            state.in_max_size = state.in_max_size.max(values.len());
        }
        FilterExpr::ArrayContains { field, .. } => {
            state.has_array_contains = true;
            *state.array_contains_fields.entry(field.join(".")).or_insert(0) += 1;
        }
        FilterExpr::ArrayContainsAny { values, .. } => {
            state.has_array_contains_any = true;
            state.array_contains_any_max_size =
                state.array_contains_any_max_size.max(values.len());
        }
        FilterExpr::And(children) | FilterExpr::Or(children) => {
            for c in children {
                walk(c, state);
            }
        }
    }
}

// Builder fn build_filter(...) -> FirestoreQueryFilter goes in Task 5.
// Allow `Literal` to be unused at this stage; Task 5 imports it.
#[allow(dead_code)]
fn _literal_marker(_l: &Literal) {}

#[cfg(test)]
mod tests {}
```

In `src/main.rs`, add `mod firestore_filter;` next to `mod firestore_error;`.

Run: `cargo build`
Expected: builds clean, possibly with dead-code warnings on `_literal_marker`.

- [ ] **Step 2: Add validation tests**

Replace `mod tests {}` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_parser::{CmpOp, FilterExpr, Literal};

    fn cmp(field: &[&str], op: CmpOp, lit: Literal) -> FilterExpr {
        FilterExpr::Compare {
            field: field.iter().map(|s| s.to_string()).collect(),
            op,
            value: lit,
        }
    }

    #[test]
    fn validates_inequality_on_one_field() {
        let expr = FilterExpr::And(vec![
            cmp(&["age"], CmpOp::Gt, Literal::Int(18)),
            cmp(&["age"], CmpOp::Lt, Literal::Int(99)),
        ]);
        assert!(validate(&expr).is_ok());
    }

    #[test]
    fn rejects_inequality_on_two_fields() {
        let expr = FilterExpr::And(vec![
            cmp(&["age"], CmpOp::Gt, Literal::Int(18)),
            cmp(&["score"], CmpOp::Lt, Literal::Int(100)),
        ]);
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("inequality on at most one field"));
        assert!(err.contains("age"));
        assert!(err.contains("score"));
    }

    #[test]
    fn rejects_in_with_31_values() {
        let values: Vec<Literal> = (0..31).map(Literal::Int).collect();
        let expr = FilterExpr::In {
            field: vec!["x".to_string()],
            values,
            negated: false,
        };
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("IN/NOT IN"));
        assert!(err.contains("30"));
        assert!(err.contains("31"));
    }

    #[test]
    fn rejects_array_contains_with_array_contains_any() {
        let expr = FilterExpr::And(vec![
            FilterExpr::ArrayContains {
                field: vec!["tags".to_string()],
                value: Literal::Str("a".into()),
            },
            FilterExpr::ArrayContainsAny {
                field: vec!["tags".to_string()],
                values: vec![Literal::Str("b".into())],
            },
        ]);
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("ARRAY_CONTAINS"));
        assert!(err.contains("ARRAY_CONTAINS_ANY"));
    }

    #[test]
    fn rejects_two_array_contains_on_same_field() {
        let expr = FilterExpr::And(vec![
            FilterExpr::ArrayContains {
                field: vec!["tags".to_string()],
                value: Literal::Str("a".into()),
            },
            FilterExpr::ArrayContains {
                field: vec!["tags".to_string()],
                value: Literal::Str("b".into()),
            },
        ]);
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("at most one ARRAY_CONTAINS"));
        assert!(err.contains("tags"));
    }

    #[test]
    fn or_branches_aggregate_for_validation() {
        // Even across OR branches, two distinct inequality fields are still rejected
        // (Firestore restriction holds regardless of conjunction).
        let expr = FilterExpr::Or(vec![
            cmp(&["age"], CmpOp::Gt, Literal::Int(18)),
            cmp(&["score"], CmpOp::Lt, Literal::Int(100)),
        ]);
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("inequality"));
    }
}
```

Run: `cargo test firestore_filter::tests`
Expected: 6 passing.

- [ ] **Step 3: Commit**

```bash
git add src/firestore_filter.rs src/main.rs
git commit -m "feat(firestore_filter): pre-flight validation of Firestore filter restrictions"
```

---

## Task 5: Firestore filter — build_filter mapper

**Files:**
- Modify: `src/firestore_filter.rs`

Maps `FilterExpr` → `firestore::FirestoreQueryFilter` via the firestore-rs fluent API. Exact API names verified at implementation time — see step 1.

- [ ] **Step 1: Determine the firestore-rs filter API surface**

Before writing the builder, confirm the actual API by reading the relevant doc:
```bash
cargo doc --no-deps -p firestore --open
```
Or grep target/doc/firestore for the `FirestoreQueryFilter`, `FirestoreQueryFilterCompare`, and `FirestoreQueryFilterComposite` types. The firestore-rs README has a fluent example like `q.filter(|q| q.field("name").eq("Alice"))` — the **fluent closure form** is the typical entry. The "raw filter" form using `FirestoreQueryFilter::Compare(...)` and `FirestoreQueryFilter::Composite(...)` may also exist.

The plan below assumes the raw-enum API. If the installed firestore-rs version only exposes the fluent-closure form, the builder restructures to:

```rust
pub fn apply_filter<'a>(
    builder: firestore::FirestoreQueryBuilder<'a>,
    expr: &FilterExpr,
) -> firestore::FirestoreQueryBuilder<'a> {
    builder.filter(|q| build_filter_in_closure(q, expr))
}
```

… and `build_filter_in_closure` recursively populates the closure's filter-builder argument. Pick whichever form the version supports.

- [ ] **Step 2: Implement the builder**

Append to `src/firestore_filter.rs` (above `#[cfg(test)] mod tests`):

```rust
use firestore::{
    FirestoreQueryFilter,
    FirestoreQueryFilterCompare,
    FirestoreQueryFilterComposite,
    FirestoreQueryFilterCompositeOperator,
};

/// Convert a Phase-2 `FilterExpr` AST into the firestore-rs filter type.
/// Pre-flight validation should have run already; this function trusts the input.
pub fn build_filter(expr: &FilterExpr) -> FirestoreQueryFilter {
    match expr {
        FilterExpr::Compare { field, op, value } => FirestoreQueryFilter::Compare(
            Some(compare_op(field.join("."), *op, literal_to_value(value))),
        ),
        FilterExpr::In { field, values, negated } => {
            let arr = literals_to_array(values);
            let cmp = if *negated {
                FirestoreQueryFilterCompare::NotIn(field.join(".").into(), arr)
            } else {
                FirestoreQueryFilterCompare::In(field.join(".").into(), arr)
            };
            FirestoreQueryFilter::Compare(Some(cmp))
        }
        FilterExpr::ArrayContains { field, value } => FirestoreQueryFilter::Compare(Some(
            FirestoreQueryFilterCompare::ArrayContains(
                field.join(".").into(),
                literal_to_value(value),
            ),
        )),
        FilterExpr::ArrayContainsAny { field, values } => {
            FirestoreQueryFilter::Compare(Some(FirestoreQueryFilterCompare::ArrayContainsAny(
                field.join(".").into(),
                literals_to_array(values),
            )))
        }
        FilterExpr::And(children) => FirestoreQueryFilter::Composite(
            FirestoreQueryFilterComposite::new(
                children.iter().map(build_filter).collect(),
                FirestoreQueryFilterCompositeOperator::And,
            ),
        ),
        FilterExpr::Or(children) => FirestoreQueryFilter::Composite(
            FirestoreQueryFilterComposite::new(
                children.iter().map(build_filter).collect(),
                FirestoreQueryFilterCompositeOperator::Or,
            ),
        ),
    }
}

fn compare_op(
    path: String,
    op: CmpOp,
    value: gcloud_sdk::google::firestore::v1::Value,
) -> FirestoreQueryFilterCompare {
    match op {
        CmpOp::Eq => FirestoreQueryFilterCompare::Equal(path.into(), value),
        CmpOp::Ne => FirestoreQueryFilterCompare::NotEqual(path.into(), value),
        CmpOp::Lt => FirestoreQueryFilterCompare::LessThan(path.into(), value),
        CmpOp::Le => FirestoreQueryFilterCompare::LessThanOrEqual(path.into(), value),
        CmpOp::Gt => FirestoreQueryFilterCompare::GreaterThan(path.into(), value),
        CmpOp::Ge => FirestoreQueryFilterCompare::GreaterThanOrEqual(path.into(), value),
    }
}

fn literal_to_value(lit: &Literal) -> gcloud_sdk::google::firestore::v1::Value {
    use gcloud_sdk::google::firestore::v1::value::ValueType as V;
    use gcloud_sdk::google::firestore::v1::Value as PV;

    let value_type = match lit {
        Literal::Str(s) => V::StringValue(s.clone()),
        Literal::Int(n) => V::IntegerValue(*n),
        Literal::Float(f) => V::DoubleValue(*f),
        Literal::Bool(b) => V::BooleanValue(*b),
        Literal::Null => V::NullValue(0),
        Literal::Timestamp(dt) => V::TimestampValue(prost_types::Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        }),
    };
    PV { value_type: Some(value_type) }
}

fn literals_to_array(lits: &[Literal]) -> gcloud_sdk::google::firestore::v1::Value {
    use gcloud_sdk::google::firestore::v1::{value::ValueType, ArrayValue, Value};
    Value {
        value_type: Some(ValueType::ArrayValue(ArrayValue {
            values: lits.iter().map(literal_to_value).collect(),
        })),
    }
}

// Drop the placeholder.
// (Remove `_literal_marker` if it's still around.)
```

Remove the dead-code marker `fn _literal_marker(_l: &Literal) {}` from Task 4. The real `literal_to_value` uses `Literal` properly.

If `prost_types::Timestamp` isn't already in the dep tree, `cargo add prost-types` (it's a small crate already used transitively by gcloud-sdk).

- [ ] **Step 3: Run cargo build**

```bash
cargo build
```

If the API names differ (e.g., `FirestoreQueryFilterCompare` is named `FirestoreFilterCompare`, or `Composite::new` takes args in different order), adapt the call sites. The contract: `build_filter` returns whatever type firestore-rs `FirestoreSelectQueryBuilder::filter()` accepts.

If gcloud_sdk re-export paths differ from `gcloud_sdk::google::firestore::v1::Value`, the imports in step 2 may need changes. Check existing usages in `src/schema_infer.rs` for the exact path your version uses.

- [ ] **Step 4: Verify tests still pass**

```bash
cargo test
```
Expected: all unit tests still pass. No new tests for the builder (it's exercised through `execute_query` in Task 7+ and via the integration test in Task 15).

- [ ] **Step 5: Commit**

```bash
git add src/firestore_filter.rs
git commit -m "feat(firestore_filter): build_filter maps FilterExpr to firestore-rs Filter"
```

---

## Task 6: Wire CURSOR_CACHE and COUNT_CACHE in state

**Files:**
- Modify: `src/state.rs`

- [ ] **Step 1: Add the two new caches**

In `src/state.rs`, after the `SCHEMA_CACHE` declaration, add:

```rust
use crate::cache::TtlLruCache;
use std::time::Duration;

/// Per-(table, where) cached row counts, populated by execute_query, evicted after 30 s
/// or when capacity exceeds 200 keys.
pub static COUNT_CACHE: Lazy<RwLock<TtlLruCache<CountKey, u64>>> = Lazy::new(|| {
    RwLock::new(TtlLruCache::new(200, Duration::from_secs(30)))
});

/// Per-(table, where, order_by) cached cursors for sequential pagination.
/// Each entry maps page-end offset to the FirestoreDocument that closes that page.
pub static CURSOR_CACHE: Lazy<RwLock<TtlLruCache<QueryKey, CursorEntry>>> = Lazy::new(|| {
    RwLock::new(TtlLruCache::new(100, Duration::from_secs(300)))
});

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct CountKey {
    pub table: String,
    pub where_canonical: String,
}

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct QueryKey {
    pub table: String,
    pub where_canonical: String,
    pub order_by_canonical: String,
}

#[derive(Clone, Debug)]
pub struct CursorEntry {
    pub cursors: std::collections::BTreeMap<u64, firestore::FirestoreDocument>,
}
```

Imports `Duration` and `TtlLruCache` may need to land at top of file too — check the existing file.

- [ ] **Step 2: Verify build**

```bash
cargo build
```
Expected: clean, with dead-code warnings on the new statics until Tasks 8/9 use them.

- [ ] **Step 3: Commit**

```bash
git add src/state.rs
git commit -m "feat(state): COUNT_CACHE and CURSOR_CACHE for query layer"
```

---

## Task 7: execute_query — integrate WHERE filter

**Files:**
- Modify: `src/handlers/query.rs`

Add WHERE-clause handling to `execute_query`. No COUNT cache yet (still uses rows.len()), no cursor pagination yet (still pure OFFSET). One step at a time.

- [ ] **Step 1: Rewrite execute_query body to use the new ParsedQuery shape**

Find the `execute_query` function in `src/handlers/query.rs`. The Phase-1 body builds `q = db.fluent().select().from(...).order_by(...).limit(...).offset(...).query()`. Update to also call `.filter(...)` if `parsed.where_clause` is present.

Replace the section that builds `q` (between parsing and `let started = Instant::now()`):

```rust
    // Pre-flight Firestore restriction validation
    if let Some(filter) = &parsed.where_clause {
        if let Err(msg) = crate::firestore_filter::validate(filter) {
            return crate::rpc::error_response(id, -32602, &msg, None);
        }
    }

    let order_items: Vec<(String, firestore::FirestoreQueryDirection)> = parsed
        .order_by
        .iter()
        .map(|i| (
            i.field.clone(),
            if i.desc { firestore::FirestoreQueryDirection::Descending }
            else      { firestore::FirestoreQueryDirection::Ascending },
        ))
        .collect();

    let mut q = db.fluent().select().from(parsed.table.as_str());

    if let Some(filter) = &parsed.where_clause {
        let firestore_filter = crate::firestore_filter::build_filter(filter);
        q = q.filter(firestore_filter);
    }

    if !order_items.is_empty() {
        q = q.order_by(order_items);
    }
    if let Some(n) = parsed.limit { q = q.limit(n as u32); }
    if let Some(o) = parsed.offset { q = q.offset(o as u32); }
```

If `q.filter(...)` doesn't compile (firestore-rs API takes a closure or different type), wrap appropriately — see Task 5's note on the closure-form alternative.

- [ ] **Step 2: Verify build**

```bash
cargo build
```
Expected: clean.

- [ ] **Step 3: Verify all tests still pass**

```bash
cargo test
```
Expected: all unit tests pass. (No new unit tests in this task — handler integration is exercised by the integration test in Task 15.)

- [ ] **Step 4: Manual smoke test**

Plug in a known Firestore project and pipe a WHERE query directly:

```bash
pkill -f 'firestore/firestore-plugin' 2>/dev/null; sleep 1
just dev-install
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"settings":{"project_id":"<your-project>"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"test_connection","params":{}}' \
  '{"jsonrpc":"2.0","id":3,"method":"execute_query","params":{"params":{},"query":"SELECT * FROM \"customers\" LIMIT 5"}}' \
  '{"jsonrpc":"2.0","id":4,"method":"execute_query","params":{"params":{},"query":"SELECT * FROM \"customers\" WHERE email = '"'"'<known-email>'"'"' LIMIT 5"}}' \
  | timeout 30 ~/.local/share/tabularis/plugins/firestore/firestore-plugin | tail -2
```
Expected: id 3 returns multiple rows, id 4 returns the one matching row (assuming the email exists).

- [ ] **Step 5: Commit**

```bash
git add src/handlers/query.rs
git commit -m "feat(query): execute_query honours WHERE filter via firestore_filter mapper"
```

---

## Task 8: execute_query — parallel COUNT cache

**Files:**
- Modify: `src/handlers/query.rs`

- [ ] **Step 1: Add the canonical-form helper**

At the bottom of `src/handlers/query.rs`, add:

```rust
/// Stable canonical form of a FilterExpr for cache keys. Two semantically equal
/// expressions produce the same string — children of And/Or are sorted, literals
/// are formatted consistently.
fn canonical_filter(expr: &crate::query_parser::FilterExpr) -> String {
    use crate::query_parser::FilterExpr as F;
    match expr {
        F::Compare { field, op, value } => {
            format!("(cmp {} {:?} {})", field.join("."), op, canonical_literal(value))
        }
        F::In { field, values, negated } => {
            let mut vs: Vec<String> = values.iter().map(canonical_literal).collect();
            vs.sort();
            format!(
                "({} {} [{}])",
                if *negated { "not_in" } else { "in" },
                field.join("."),
                vs.join(",")
            )
        }
        F::ArrayContains { field, value } => {
            format!("(ac {} {})", field.join("."), canonical_literal(value))
        }
        F::ArrayContainsAny { field, values } => {
            let mut vs: Vec<String> = values.iter().map(canonical_literal).collect();
            vs.sort();
            format!("(aca {} [{}])", field.join("."), vs.join(","))
        }
        F::And(children) => {
            let mut parts: Vec<String> = children.iter().map(canonical_filter).collect();
            parts.sort();
            format!("(and {})", parts.join(" "))
        }
        F::Or(children) => {
            let mut parts: Vec<String> = children.iter().map(canonical_filter).collect();
            parts.sort();
            format!("(or {})", parts.join(" "))
        }
    }
}

fn canonical_literal(lit: &crate::query_parser::Literal) -> String {
    use crate::query_parser::Literal as L;
    match lit {
        L::Str(s) => format!("'{s}'"),
        L::Int(n) => n.to_string(),
        L::Float(f) => format!("{f:?}"), // Debug fmt is stable across runs for a given f64
        L::Bool(b) => b.to_string(),
        L::Null => "null".to_string(),
        L::Timestamp(dt) => format!("ts:{}", dt.to_rfc3339()),
    }
}

fn canonical_order_by(items: &[crate::query_parser::OrderItem]) -> String {
    items
        .iter()
        .map(|i| format!("{} {}", i.field, if i.desc { "DESC" } else { "ASC" }))
        .collect::<Vec<_>>()
        .join(", ")
}
```

- [ ] **Step 2: Add count + parallel join to execute_query**

Find the section in `execute_query` that does:
```rust
    let started = std::time::Instant::now();
    let docs: Vec<firestore::FirestoreDocument> = match q.query().await {
        Ok(d) => d,
        Err(e) => return error_from_query(id, &e),
    };
    let elapsed = started.elapsed().as_millis() as u64;
```

Replace with:

```rust
    // Build the count query (same filter, no order/limit/offset).
    let count_key = crate::state::CountKey {
        table: parsed.table.clone(),
        where_canonical: parsed
            .where_clause
            .as_ref()
            .map(canonical_filter)
            .unwrap_or_default(),
    };

    let count_key = crate::state::CountKey {
        table: parsed.table.clone(),
        where_canonical: parsed
            .where_clause
            .as_ref()
            .map(canonical_filter)
            .unwrap_or_default(),
    };

    let cached_count: Option<u64> = crate::state::COUNT_CACHE
        .write()
        .unwrap()
        .get(&count_key)
        .copied();

    let started = std::time::Instant::now();

    let (docs_result, count_result) = if cached_count.is_some() {
        // Cache hit — only run the data query
        let d = q.query().await;
        (d, Ok(cached_count.unwrap()))
    } else {
        // Build a separate count query with the same filter.
        let mut count_q = db.fluent().select().from(parsed.table.as_str());
        if let Some(filter) = &parsed.where_clause {
            count_q = count_q.filter(crate::firestore_filter::build_filter(filter));
        }
        // Two parallel awaits.
        tokio::join!(q.query(), count_q.count())
    };

    let elapsed = started.elapsed().as_millis() as u64;

    let docs: Vec<firestore::FirestoreDocument> = match docs_result {
        Ok(d) => d,
        Err(e) => return error_from_query(id, &e),
    };

    let total_count: u64 = match count_result {
        Ok(n) => {
            crate::state::COUNT_CACHE
                .write()
                .unwrap()
                .insert(count_key, n);
            n
        }
        Err(e) => {
            // Count failure is non-fatal. Fall back to docs.len().
            eprintln!("count failed (falling back to rows.len): {e}");
            docs.len() as u64
        }
    };
```

Note `count_q.count()` — confirm the actual method name on `FirestoreSelectQueryBuilder`. Likely `.count()` returning `Result<u64, FirestoreError>` (sometimes wrapped in another struct that has `.count()` too).

Then update the response builder at the end of `execute_query`:

```rust
    ok_response(
        id,
        json!({
            "columns": column_names,
            "rows": rows,
            "total_count": total_count,        // <-- was rows.len()
            "affected_rows": 0,
            "execution_time_ms": elapsed,
        }),
    )
```

- [ ] **Step 3: Verify build**

```bash
cargo build
```
Expected: clean. If `count_q.count()` doesn't compile, see firestore-rs's aggregation example — the API may be `count_q.count().await` or `count_q.aggregate(.count()).await` or similar.

- [ ] **Step 4: Verify tests still pass**

```bash
cargo test
```
Expected: all unit tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/handlers/query.rs
git commit -m "feat(query): parallel COUNT aggregation with TTL cache for total_count"
```

---

## Task 9: execute_query — cursor pagination

**Files:**
- Modify: `src/handlers/query.rs`

Adds cursor-based pagination for sequential next-page navigation. Falls back to OFFSET for jump-to-page.

- [ ] **Step 1: Resolve effective pagination from params + SQL**

At the top of `execute_query` (after parsing the SQL), add params-driven pagination:

```rust
    // Resolve effective limit/offset: params.page/page_size override SQL.
    let host_page_size: Option<u64> = params.get("page_size").and_then(Value::as_u64);
    let host_page: Option<u64> = params.get("page").and_then(Value::as_u64);

    let effective_limit: u64 = host_page_size
        .or(parsed.limit)
        .unwrap_or(100);
    let effective_offset: u64 = match host_page {
        Some(p) if p > 1 => (p - 1) * effective_limit,
        _ => parsed.offset.unwrap_or(0),
    };
```

Replace the prior `if let Some(n) = parsed.limit { q = q.limit(n as u32); }` and `if let Some(o) = parsed.offset { ... }` with the new effective values:

```rust
    q = q.limit(effective_limit as u32);
```

(OFFSET goes in the cursor branch below — don't unconditionally apply it.)

- [ ] **Step 2: Add cursor lookup and apply to the query**

After the order_by application, add cursor handling:

```rust
    let query_key = crate::state::QueryKey {
        table: parsed.table.clone(),
        where_canonical: parsed
            .where_clause
            .as_ref()
            .map(canonical_filter)
            .unwrap_or_default(),
        order_by_canonical: canonical_order_by(&parsed.order_by),
    };

    let cursor_for_offset: Option<firestore::FirestoreDocument> = if effective_offset > 0 {
        crate::state::CURSOR_CACHE
            .write()
            .unwrap()
            .get(&query_key)
            .and_then(|entry| entry.cursors.get(&effective_offset).cloned())
    } else {
        None
    };

    if let Some(cursor) = &cursor_for_offset {
        // start_after the cached document
        q = q.start_after(cursor.clone());
    } else if effective_offset > 0 {
        q = q.offset(effective_offset as u32);
    }
```

`q.start_after(...)` — confirm the firestore-rs method name. May be `q.start_after(cursor_doc)` or `q.cursor(StartAfter, cursor)`. Use whatever the rustdoc shows.

- [ ] **Step 3: After data query succeeds, write cursor for next page**

After `let docs: Vec<...> = ...` is populated:

```rust
    // Update cursor cache: store the last doc as the cursor for offset = effective_offset + docs.len()
    if let Some(last_doc) = docs.last() {
        let next_offset = effective_offset + docs.len() as u64;
        let mut cache = crate::state::CURSOR_CACHE.write().unwrap();
        if let Some(entry) = cache.get(&query_key) {
            let mut new_cursors = entry.cursors.clone();
            new_cursors.insert(next_offset, last_doc.clone());
            cache.insert(query_key.clone(), crate::state::CursorEntry { cursors: new_cursors });
        } else {
            let mut cursors = std::collections::BTreeMap::new();
            cursors.insert(next_offset, last_doc.clone());
            cache.insert(query_key, crate::state::CursorEntry { cursors });
        }
    }
```

- [ ] **Step 4: Verify build**

```bash
cargo build
```

If `start_after` is named differently or takes a different argument shape, fix the call site. The contract: pass a Firestore document the query should resume after.

- [ ] **Step 5: Verify tests**

```bash
cargo test
```
Expected: all unit tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/handlers/query.rs
git commit -m "feat(query): cursor-based pagination with OFFSET fallback for jump-to-page"
```

---

## Task 10: schema_infer — references field for ER diagram

**Files:**
- Modify: `src/schema_infer.rs`

Adds the `references: Option<String>` field to `ColumnInfo`. Reference-target extraction looks at the resource path of any Reference-valued field during inference.

- [ ] **Step 1: Add the field to ColumnInfo**

In `src/schema_infer.rs`, find the `ColumnInfo` struct:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
}
```

Replace with:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub references: Option<String>,
}
```

- [ ] **Step 2: Update the synthetic __id__ column in `infer`**

Find the line in `infer` that creates the __id__ row:

```rust
    let mut out = vec![ColumnInfo {
        name: "__id__".into(),
        data_type: "string".into(),
        is_nullable: false,
    }];
```

Replace with:

```rust
    let mut out = vec![ColumnInfo {
        name: "__id__".into(),
        data_type: "string".into(),
        is_nullable: false,
        references: None,
    }];
```

- [ ] **Step 3: Pass the references field in field-loop output**

Find the iteration in `infer`:
```rust
    for (name, types) in types_by_field {
        let (data_type, has_null) = classify_set(&types);
        let missing = seen_count.get(&name).map_or(true, |&c| c < total);
        let is_nullable = has_null || missing;
        out.push(ColumnInfo { name, data_type, is_nullable });
    }
```

Replace with:

```rust
    for (name, types) in types_by_field {
        let (data_type, has_null) = classify_set(&types);
        let missing = seen_count.get(&name).map_or(true, |&c| c < total);
        let is_nullable = has_null || missing;
        let references = reference_targets_by_field
            .get(&name)
            .and_then(|targets| {
                if targets.len() == 1 { targets.iter().next().cloned() } else { None }
            });
        out.push(ColumnInfo { name, data_type, is_nullable, references });
    }
```

And add tracking of reference targets at the top of the function. Find the existing `let mut types_by_field: ...` line and add alongside it:

```rust
    let mut reference_targets_by_field: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
```

Then change `infer`'s signature: it currently takes `&[DocumentTypes]`. We need access to the actual reference values to extract targets. Either:
- Pass an additional parameter `&[BTreeMap<String, Option<String>>]` (per-doc reference targets), OR
- Change `DocumentTypes` to carry the optional reference target alongside the FieldType.

The cleaner change: alongside `DocumentTypes`, accept a `Vec<BTreeMap<String, String>>` of reference-targets-by-field where each map entry indicates the target collection of any Reference-valued field in that doc.

Update the type alias and `infer` signature:

```rust
pub type DocumentTypes = BTreeMap<String, FieldType>;

/// Per-document, the optional reference target collection for any Reference-typed field.
/// Only populated for fields where classify_value() returned Reference.
pub type DocumentReferences = BTreeMap<String, String>;

pub fn infer(sample: &[DocumentTypes], references: &[DocumentReferences]) -> Vec<ColumnInfo> {
```

Inside `infer`, between the docs walk that builds `types_by_field` and the column-emit loop, add:

```rust
    for refs in references {
        for (k, target) in refs {
            reference_targets_by_field
                .entry(k.clone())
                .or_default()
                .insert(target.clone());
        }
    }
```

- [ ] **Step 4: Update existing tests for the new `infer` signature**

All existing tests in `schema_infer::tests` call `infer(&sample)`. Change them to `infer(&sample, &[])` (no reference data). Find every `infer(&sample)` and `infer(&[])` call and pass an empty `&[]` as the second argument.

Also, the helper `doc()` in tests still produces only `DocumentTypes`. Add a parallel helper for the references map:

```rust
    fn refs(pairs: &[(&str, &str)]) -> DocumentReferences {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
```

- [ ] **Step 5: Add tests for reference extraction**

Append to `mod tests`:

```rust
    #[test]
    fn reference_value_extracts_target_collection() {
        let sample = vec![doc(&[("author", FieldType::Reference)])];
        let refs = vec![refs(&[("author", "users")])];
        let cols = infer(&sample, &refs);
        let author = cols.iter().find(|c| c.name == "author").unwrap();
        assert_eq!(author.data_type, "reference");
        assert_eq!(author.references, Some("users".to_string()));
    }

    #[test]
    fn mixed_reference_targets_yield_no_fk() {
        let sample = vec![
            doc(&[("ref_field", FieldType::Reference)]),
            doc(&[("ref_field", FieldType::Reference)]),
        ];
        let refs = vec![
            refs(&[("ref_field", "users")]),
            refs(&[("ref_field", "advisors")]),
        ];
        let cols = infer(&sample, &refs);
        let f = cols.iter().find(|c| c.name == "ref_field").unwrap();
        assert_eq!(f.references, None);
    }

    #[test]
    fn no_reference_data_yields_no_fk() {
        // Field type is Reference but no target was extracted (refs empty).
        let sample = vec![doc(&[("author", FieldType::Reference)])];
        let cols = infer(&sample, &[]);
        let f = cols.iter().find(|c| c.name == "author").unwrap();
        assert_eq!(f.references, None);
    }
```

- [ ] **Step 6: Update all callers of `infer()` outside the tests**

The Phase-1 callers are in:
- `src/handlers/metadata.rs` — `get_columns` and `get_all_columns_batch`
- `src/handlers/query.rs` — the cache-miss fallback in `execute_query`

For now, pass `&[]` as the second argument at every call site. Reference extraction in the actual handlers comes in Task 12 alongside `get_schema_snapshot`.

Search for `crate::schema_infer::infer(&sample)` and replace with `crate::schema_infer::infer(&sample, &[])`.

- [ ] **Step 7: Update to_json to include references**

In the `ColumnInfo::to_json` method:

```rust
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "data_type": self.data_type,
            "is_nullable": self.is_nullable,
            "default_value": Value::Null,
            "is_pk": self.name == "__id__",
            "is_auto_increment": false,
            "character_maximum_length": Value::Null,
            "comment": if self.name == "__id__" { Value::String("Firestore document ID".into()) } else { Value::Null },
            "references": self.references.as_ref().map(|s| Value::String(s.clone())).unwrap_or(Value::Null),
        })
    }
```

- [ ] **Step 8: Verify build + tests**

```bash
cargo build && cargo test
```
Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/schema_infer.rs src/handlers/metadata.rs src/handlers/query.rs
git commit -m "feat(schema_infer): track reference targets per field for ER diagram"
```

---

## Task 11: schema_infer — native JSON for Maps and Arrays

**Files:**
- Modify: `src/schema_infer.rs`

The smoke-test gate from the spec runs at the end of this task.

- [ ] **Step 1: Switch the Map and Array arms to native JSON**

In `src/schema_infer.rs`, find `serialize_value`. It currently has:

```rust
        Some(V::ArrayValue(a)) => {
            let items: Vec<Value> = a.values.iter().map(serialize_value).collect();
            Value::String(serde_json::to_string(&items).unwrap_or_default())
        }
        Some(V::MapValue(m)) => {
            let map: serde_json::Map<String, Value> = m
                .fields
                .iter()
                .map(|(k, x)| (k.clone(), serialize_value(x)))
                .collect();
            Value::String(serde_json::to_string(&Value::Object(map)).unwrap_or_default())
        }
```

Replace with:

```rust
        Some(V::ArrayValue(a)) => {
            Value::Array(a.values.iter().map(serialize_value).collect())
        }
        Some(V::MapValue(m)) => {
            Value::Object(m.fields.iter().map(|(k, x)| (k.clone(), serialize_value(x))).collect())
        }
```

- [ ] **Step 2: Build, install, smoke-test**

```bash
cargo build
pkill -f 'firestore/firestore-plugin' 2>/dev/null; sleep 2
just dev-install
```

In Tabularis: disconnect the Firestore connection, reconnect, double-click a collection that has map or array fields (e.g. `customers` if `address` is a map). Observe the cell rendering.

**Expected outcomes:**
- Tabularis renders the nested JSON sensibly (expandable tree, pretty-printed JSON in cell, or any other non-broken rendering) → keep the change, proceed to step 3.
- Tabularis shows `[object Object]` or empty cells or crashes → revert this commit's `serialize_value` arms and ship Phase 2 with stringified maps. Skip step 3.

- [ ] **Step 3: Commit (only if smoke-test passed)**

```bash
git add src/schema_infer.rs
git commit -m "feat(schema_infer): native JSON for maps and arrays in row payloads"
```

If the smoke-test failed, **don't commit**. Run `git checkout src/schema_infer.rs` to revert and document the result in the Phase 2 verification step (Task 16).

---

## Task 12: get_schema_snapshot — populate FK relationships

**Files:**
- Modify: `src/handlers/metadata.rs`
- Modify: `src/schema_infer.rs`

- [ ] **Step 1: Extend types_from_document to also produce reference targets**

In `src/schema_infer.rs`, find `types_from_document`. It currently returns `DocumentTypes`. Add a parallel function:

```rust
/// Walk one document and extract reference targets for any Reference-typed field.
/// The target is the collection segment immediately after `documents/` in the
/// reference's resource path.
pub fn references_from_document(
    doc: &gcloud_sdk::google::firestore::v1::Document,
) -> DocumentReferences {
    use gcloud_sdk::google::firestore::v1::value::ValueType as V;
    let mut out = DocumentReferences::new();
    for (name, val) in &doc.fields {
        if let Some(V::ReferenceValue(path)) = val.value_type.as_ref() {
            // path = "projects/<p>/databases/<d>/documents/<col>/<doc>[/<sub>/<doc>]*"
            if let Some(target) = extract_target_collection(path) {
                out.insert(name.clone(), target);
            }
        }
    }
    out
}

fn extract_target_collection(resource_path: &str) -> Option<String> {
    // Find "documents/" then take the segment immediately after.
    let idx = resource_path.find("/documents/")?;
    let after = &resource_path[idx + "/documents/".len()..];
    after.split('/').next().map(str::to_string)
}

#[cfg(test)]
mod resource_path_tests {
    use super::*;

    #[test]
    fn extracts_root_collection() {
        let path = "projects/p/databases/(default)/documents/users/abc123";
        assert_eq!(extract_target_collection(path), Some("users".to_string()));
    }

    #[test]
    fn handles_subcollection_doc() {
        let path = "projects/p/databases/(default)/documents/users/abc/orders/xyz";
        assert_eq!(extract_target_collection(path), Some("users".to_string()));
    }

    #[test]
    fn returns_none_for_unrecognised_path() {
        assert_eq!(extract_target_collection("garbage"), None);
    }
}
```

- [ ] **Step 2: Implement get_schema_snapshot**

In `src/handlers/metadata.rs`, replace `get_schema_snapshot`:

```rust
pub async fn get_schema_snapshot(id: Value, _params: &Value) -> Value {
    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    // List all root collections.
    let stream = match db
        .fluent()
        .list()
        .collections()
        .stream_all_with_errors()
        .await
    {
        Ok(s) => s,
        Err(e) => return error_from(id, &e),
    };

    use futures::TryStreamExt;
    let table_names: Vec<String> = match stream.try_collect().await {
        Ok(v) => v,
        Err(e) => return error_from(id, &e),
    };

    let n = crate::state::settings().map(|s| s.sample_size).unwrap_or(50);

    // Parallel fetch for every collection.
    let fetches = table_names.iter().cloned().map(|table| {
        let db = db.clone();
        async move {
            let docs: Vec<firestore::FirestoreDocument> = db
                .fluent()
                .select()
                .from(table.as_str())
                .limit(n)
                .query()
                .await
                .unwrap_or_default();
            let types: Vec<crate::schema_infer::DocumentTypes> = docs
                .iter()
                .map(crate::schema_infer::types_from_document)
                .collect();
            let refs: Vec<crate::schema_infer::DocumentReferences> = docs
                .iter()
                .map(crate::schema_infer::references_from_document)
                .collect();
            let columns = crate::schema_infer::infer(&types, &refs);
            (table, columns)
        }
    });
    let fetched: Vec<(String, Vec<crate::schema_infer::ColumnInfo>)> =
        futures::future::join_all(fetches).await;

    // Assemble the response envelope.
    let mut tables_json: Vec<Value> = Vec::new();
    let mut columns_json = serde_json::Map::new();
    let mut foreign_keys_json = serde_json::Map::new();

    for (table, columns) in fetched {
        tables_json.push(json!({
            "name": table,
            "schema": Value::Null,
            "comment": Value::Null
        }));
        let cols_arr: Vec<Value> = columns.iter().map(|c| c.to_json()).collect();
        columns_json.insert(table.clone(), Value::Array(cols_arr));

        let fks: Vec<Value> = columns
            .iter()
            .filter_map(|c| {
                c.references.as_ref().map(|target| {
                    json!({
                        "from_column": c.name.clone(),
                        "to_table": target.clone(),
                        "to_column": "__id__"
                    })
                })
            })
            .collect();
        if !fks.is_empty() {
            foreign_keys_json.insert(table, Value::Array(fks));
        }
    }

    tables_json.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

    ok_response(
        id,
        json!({
            "tables": tables_json,
            "columns": Value::Object(columns_json),
            "foreign_keys": Value::Object(foreign_keys_json)
        }),
    )
}
```

The function is async. Make sure rpc.rs dispatches with `.await`:

```rust
        "get_schema_snapshot" => handlers::metadata::get_schema_snapshot(id, &params).await,
```

(If it currently doesn't, add the `.await` in the dispatch.)

- [ ] **Step 3: Update get_all_columns_batch to also pass references**

In `src/handlers/metadata.rs`, find the `get_all_columns_batch` async fn. It currently calls `infer(&sample)` without references. Update the call site:

```rust
            let docs: Vec<firestore::FirestoreDocument> = db
                .fluent()
                .select()
                .from(table.as_str())
                .limit(n)
                .query()
                .await
                .unwrap_or_default();
            let sample: Vec<crate::schema_infer::DocumentTypes> = docs
                .iter()
                .map(crate::schema_infer::types_from_document)
                .collect();
            let refs: Vec<crate::schema_infer::DocumentReferences> = docs
                .iter()
                .map(crate::schema_infer::references_from_document)
                .collect();
            (table, crate::schema_infer::infer(&sample, &refs))
```

Same update for `get_columns` (single-table path).

- [ ] **Step 4: Verify build + tests**

```bash
cargo build && cargo test
```
Expected: all tests pass; the new `resource_path_tests` adds 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/handlers/metadata.rs src/schema_infer.rs
git commit -m "feat(metadata): get_schema_snapshot with inferred FK relationships"
```

---

## Task 13: explain_query — real implementation

**Files:**
- Modify: `src/handlers/query.rs`

- [ ] **Step 1: Replace the not_implemented stub**

In `src/handlers/query.rs`, find:

```rust
pub fn explain_query(id: Value, _params: &Value) -> Value {
    not_implemented(id, "explain_query")
}
```

Replace with:

```rust
pub async fn explain_query(id: Value, params: &Value) -> Value {
    let sql = params
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let parsed = match crate::query_parser::parse(&sql) {
        Ok(p) => p,
        Err(e) => return crate::rpc::error_response(id, -32602, &e, None),
    };

    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    if let Some(filter) = &parsed.where_clause {
        if let Err(msg) = crate::firestore_filter::validate(filter) {
            return crate::rpc::error_response(id, -32602, &msg, None);
        }
    }

    let mut q = db.fluent().select().from(parsed.table.as_str());
    if let Some(filter) = &parsed.where_clause {
        q = q.filter(crate::firestore_filter::build_filter(filter));
    }

    let order_items: Vec<(String, firestore::FirestoreQueryDirection)> = parsed
        .order_by
        .iter()
        .map(|i| (
            i.field.clone(),
            if i.desc { firestore::FirestoreQueryDirection::Descending }
            else      { firestore::FirestoreQueryDirection::Ascending },
        ))
        .collect();
    if !order_items.is_empty() {
        q = q.order_by(order_items);
    }
    if let Some(n) = parsed.limit { q = q.limit(n as u32); }

    match q.explain().await {
        Ok(plan) => crate::rpc::ok_response(id, json!({
            "plan_text": format!("{plan:#?}"),
            "documents_returned": plan.execution_stats.docs_returned,
            "documents_scanned": plan.execution_stats.docs_scanned,
            "index_used": plan.execution_stats.index_used,
            "execution_duration_ms": plan.execution_stats.duration_ms,
        })),
        Err(e) => error_from_query(id, &e),
    }
}
```

In `src/rpc.rs`, the dispatch entry currently calls explain_query as sync. Add `.await`:

```rust
        "explain_query" => handlers::query::explain_query(id, &params).await,
```

- [ ] **Step 2: Verify build**

```bash
cargo build
```

If the firestore-rs explain API has different field names (`docs_returned` vs `documents_returned`, `duration_ms` vs `execution_duration_ms`), check the rustdoc and adapt the field accesses. The contract: `q.explain()` returns a future that yields query-plan info; we forward the relevant fields as JSON.

- [ ] **Step 3: Commit**

```bash
git add src/handlers/query.rs src/rpc.rs
git commit -m "feat(query): explain_query returns Firestore query plan"
```

---

## Task 14: firestore_error — four new ErrorKind variants

**Files:**
- Modify: `src/firestore_error.rs`

- [ ] **Step 1: Extend ErrorKind enum**

In `src/firestore_error.rs`, find:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    FailedPrecondition,
    Unauthenticated,
    NotFound,
    Other,
}
```

Replace with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    FailedPrecondition,
    Unauthenticated,
    NotFound,
    PermissionDenied,
    ResourceExhausted,
    DeadlineExceeded,
    Unavailable,
    Other,
}
```

- [ ] **Step 2: Extend the classifier**

Find `classify`:

```rust
fn classify(raw: &str) -> ErrorKind {
    if raw.contains("FAILED_PRECONDITION") { ErrorKind::FailedPrecondition }
    else if raw.contains("UNAUTHENTICATED") { ErrorKind::Unauthenticated }
    else if raw.contains("NOT_FOUND")       { ErrorKind::NotFound }
    else                                    { ErrorKind::Other }
}
```

Replace with:

```rust
fn classify(raw: &str) -> ErrorKind {
    if      raw.contains("FAILED_PRECONDITION")  { ErrorKind::FailedPrecondition }
    else if raw.contains("UNAUTHENTICATED")      { ErrorKind::Unauthenticated }
    else if raw.contains("PERMISSION_DENIED")    { ErrorKind::PermissionDenied }
    else if raw.contains("NOT_FOUND")            { ErrorKind::NotFound }
    else if raw.contains("RESOURCE_EXHAUSTED")   { ErrorKind::ResourceExhausted }
    else if raw.contains("DEADLINE_EXCEEDED")    { ErrorKind::DeadlineExceeded }
    else if raw.contains("UNAVAILABLE")          { ErrorKind::Unavailable }
    else                                         { ErrorKind::Other }
}
```

- [ ] **Step 3: Extend map_message**

Find `map_message`. After the `if kind == ErrorKind::NotFound` block, add:

```rust
    if kind == ErrorKind::PermissionDenied {
        let project = crate::state::settings()
            .map(|s| s.project_id.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("the configured project");
        return (
            -32602,
            format!(
                "Access denied: {raw}. Check the service account's IAM roles for project '{project}' \
                 (needs at minimum 'roles/datastore.viewer' for reads)."
            ),
            None,
        );
    }
    if kind == ErrorKind::ResourceExhausted {
        return (
            -32603,
            format!(
                "Firestore quota exceeded: {raw}. Wait a minute and retry. \
                 If this persists, check the GCP Quotas page for your project."
            ),
            None,
        );
    }
    if kind == ErrorKind::DeadlineExceeded {
        return (
            -32603,
            format!(
                "Request timed out: {raw}. The query may be missing an index or scanning a very \
                 large collection — try LIMIT to narrow the result set."
            ),
            None,
        );
    }
    if kind == ErrorKind::Unavailable {
        return (
            -32603,
            format!(
                "Firestore temporarily unavailable: {raw}. \
                 This is usually transient — retry in a few seconds."
            ),
            None,
        );
    }
```

- [ ] **Step 4: Add tests**

Append to `mod tests`:

```rust
    #[test]
    fn permission_denied_message_includes_project_id_and_role_hint() {
        // SETTINGS may not be initialised in test context; fallback string is acceptable.
        let (code, msg, _) = map_message("PERMISSION_DENIED: missing scope", ErrorKind::PermissionDenied);
        assert_eq!(code, -32602);
        assert!(msg.contains("Access denied"));
        assert!(msg.contains("roles/datastore.viewer"));
    }

    #[test]
    fn resource_exhausted_message_mentions_quota() {
        let (code, msg, _) = map_message("RESOURCE_EXHAUSTED: quota", ErrorKind::ResourceExhausted);
        assert_eq!(code, -32603);
        assert!(msg.contains("quota exceeded"));
    }

    #[test]
    fn deadline_exceeded_message_suggests_limit() {
        let (code, msg, _) = map_message("DEADLINE_EXCEEDED", ErrorKind::DeadlineExceeded);
        assert_eq!(code, -32603);
        assert!(msg.contains("LIMIT"));
    }

    #[test]
    fn unavailable_message_says_transient() {
        let (code, msg, _) = map_message("UNAVAILABLE: temp", ErrorKind::Unavailable);
        assert_eq!(code, -32603);
        assert!(msg.contains("transient"));
    }

    #[test]
    fn classifier_recognises_new_status_tokens() {
        assert_eq!(classify("rpc error: code = PERMISSION_DENIED"), ErrorKind::PermissionDenied);
        assert_eq!(classify("rpc error: code = RESOURCE_EXHAUSTED"), ErrorKind::ResourceExhausted);
        assert_eq!(classify("rpc error: code = DEADLINE_EXCEEDED"), ErrorKind::DeadlineExceeded);
        assert_eq!(classify("rpc error: code = UNAVAILABLE"), ErrorKind::Unavailable);
    }
```

- [ ] **Step 5: Verify**

```bash
cargo test firestore_error::tests
```
Expected: 8 + 5 = 13 passing.

- [ ] **Step 6: Commit**

```bash
git add src/firestore_error.rs
git commit -m "feat(firestore_error): PERMISSION_DENIED/RESOURCE_EXHAUSTED/DEADLINE_EXCEEDED/UNAVAILABLE"
```

---

## Task 15: Integration test extension

**Files:**
- Modify: `tests/firestore_emulator.rs`
- Create: `tests/fixtures/seed.sh`

- [ ] **Step 1: Write the seed script**

Create `tests/fixtures/seed.sh`:

```bash
#!/usr/bin/env bash
# Seed the local Firestore emulator with Phase 2 test fixtures.
#
# Requires: gcloud emulators firestore start --host-port=localhost:8080
# Usage:    FIRESTORE_EMULATOR_HOST=localhost:8080 bash tests/fixtures/seed.sh

set -euo pipefail

HOST="${FIRESTORE_EMULATOR_HOST:-localhost:8080}"
PROJECT="${FIRESTORE_TEST_PROJECT:-demo-project}"
BASE="http://$HOST/v1/projects/$PROJECT/databases/(default)/documents"

# Helper: PATCH a document (creates if missing).
write_doc() {
    local collection="$1"
    local doc_id="$2"
    local body="$3"
    curl -fsS -X PATCH \
        "$BASE/$collection/$doc_id" \
        -H "Content-Type: application/json" \
        -d "$body" > /dev/null
}

# users
write_doc users alice '{
  "fields": {
    "email":  { "stringValue": "alice@x.de" },
    "active": { "booleanValue": true },
    "region": { "stringValue": "eu" },
    "tags":   { "arrayValue": { "values": [{"stringValue":"vip"}] } },
    "address": { "mapValue": { "fields": {
      "city":    { "stringValue": "Berlin" },
      "country": { "stringValue": "DE" }
    }}}
  }
}'

write_doc users bob '{
  "fields": {
    "email":  { "stringValue": "bob@x.de" },
    "active": { "booleanValue": false },
    "region": { "stringValue": "us" },
    "tags":   { "arrayValue": { "values": [{"stringValue":"early"}] } }
  }
}'

# posts (with reference to users)
write_doc posts post1 "$(cat <<EOF
{
  "fields": {
    "title":  { "stringValue": "Hello" },
    "views":  { "integerValue": "150" },
    "status": { "stringValue": "published" },
    "tags":   { "arrayValue": { "values": [{"stringValue":"launch"}, {"stringValue":"news"}] } },
    "priority": { "stringValue": "high" },
    "author": { "referenceValue": "projects/$PROJECT/databases/(default)/documents/users/alice" }
  }
}
EOF
)"

write_doc posts post2 "$(cat <<EOF
{
  "fields": {
    "title":  { "stringValue": "Followup" },
    "views":  { "integerValue": "50" },
    "status": { "stringValue": "draft" },
    "tags":   { "arrayValue": { "values": [{"stringValue":"draft"}] } },
    "priority": { "stringValue": "low" },
    "author": { "referenceValue": "projects/$PROJECT/databases/(default)/documents/users/bob" }
  }
}
EOF
)"

echo "seeded users (2 docs) and posts (2 docs)"
```

Make it executable:
```bash
chmod +x tests/fixtures/seed.sh
```

- [ ] **Step 2: Extend the integration test**

In `tests/firestore_emulator.rs`, after the existing `end_to_end_against_emulator` test, append:

```rust
#[test]
#[ignore]
fn phase2_query_layer_against_emulator() {
    let host = emulator_host().expect("FIRESTORE_EMULATOR_HOST not set");
    let project = std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let mut p = Plugin::spawn();

    // Initialize and connect.
    let init = p.call("initialize", json!({
        "settings": { "project_id": project, "emulator_host": host, "sample_size": 50 }
    }));
    assert!(init.get("error").is_none(), "initialize failed: {init}");

    let test = p.call("test_connection", json!({ "params": {} }));
    assert_eq!(test["result"]["success"], Value::Bool(true), "test_connection: {test}");

    // WHERE eq
    let q1 = p.call("execute_query", json!({
        "params": {},
        "query": "SELECT * FROM \"users\" WHERE email = 'alice@x.de'"
    }));
    let rows1 = q1["result"]["rows"].as_array().unwrap();
    assert_eq!(rows1.len(), 1, "alice should match: {q1}");

    // WHERE int comparison + IN
    let q2 = p.call("execute_query", json!({
        "params": {},
        "query": "SELECT * FROM \"posts\" WHERE views > 100 AND status IN ('published', 'draft')"
    }));
    let rows2 = q2["result"]["rows"].as_array().unwrap();
    assert!(rows2.len() >= 1, "expected post1: {q2}");

    // OR with parens + ARRAY_CONTAINS
    let q3 = p.call("execute_query", json!({
        "params": {},
        "query": "SELECT * FROM \"posts\" WHERE (priority = 'high' OR priority = 'urgent') AND ARRAY_CONTAINS(tags, 'launch')"
    }));
    assert!(q3.get("error").is_none(), "OR + ARRAY_CONTAINS failed: {q3}");

    // total_count
    let q4 = p.call("execute_query", json!({
        "params": {},
        "query": "SELECT * FROM \"users\" LIMIT 1"
    }));
    let total = q4["result"]["total_count"].as_u64().unwrap();
    assert_eq!(total, 2, "expected 2 seeded users: {q4}");

    // Pagination — first 1 row, then next 1 row
    let q5a = p.call("execute_query", json!({
        "params": {}, "query": "SELECT * FROM \"users\" ORDER BY email ASC LIMIT 1 OFFSET 0"
    }));
    let q5b = p.call("execute_query", json!({
        "params": {}, "query": "SELECT * FROM \"users\" ORDER BY email ASC LIMIT 1 OFFSET 1"
    }));
    let id_a = q5a["result"]["rows"][0][0].as_str().unwrap().to_string();
    let id_b = q5b["result"]["rows"][0][0].as_str().unwrap().to_string();
    assert_ne!(id_a, id_b, "pagination should produce disjoint pages: {q5a} {q5b}");

    // get_schema_snapshot — posts.author should reference users
    let snap = p.call("get_schema_snapshot", json!({ "params": {} }));
    let fks = &snap["result"]["foreign_keys"]["posts"];
    assert!(fks.is_array(), "expected posts foreign_keys array: {snap}");
    let fk_to_users = fks.as_array().unwrap().iter().find(|fk| {
        fk["from_column"] == "author" && fk["to_table"] == "users"
    });
    assert!(fk_to_users.is_some(), "expected posts.author -> users FK: {snap}");
}
```

- [ ] **Step 3: Verify the integration test is gated**

```bash
cargo test --test firestore_emulator
```
Expected: `2 ignored` (or whatever count includes the new test).

- [ ] **Step 4: Run with the emulator**

(Requires running emulator + `bash tests/fixtures/seed.sh` to seed, then:)

```bash
FIRESTORE_EMULATOR_HOST=localhost:8080 cargo test --test firestore_emulator -- --ignored
```

If you don't have an emulator handy, skip this step — CI in Phase 5 runs it. The committed test code is what matters.

- [ ] **Step 5: Commit**

```bash
git add tests/firestore_emulator.rs tests/fixtures/seed.sh
git commit -m "test(integration): Phase 2 emulator-gated test for filters, COUNT, pagination, FK"
```

---

## Task 16: Final verification + CLAUDE.md + ROADMAP update

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/ROADMAP.md`

- [ ] **Step 1: Lint clean**

```bash
cargo clippy --all-targets -- -D warnings
```
Expected: no warnings. Fix anything inline; common items: dead-code on the `NegNumber` token variant from Task 2 (delete if still unused), unused imports.

- [ ] **Step 2: Format**

```bash
cargo fmt --all -- --check
```
If diffs reported, run `cargo fmt --all` and commit the formatting changes separately.

- [ ] **Step 3: Full test suite**

```bash
cargo test
```
Expected: 70+ unit tests pass; integration tests `2 ignored`.

- [ ] **Step 4: Update CLAUDE.md**

In `CLAUDE.md`, find the "What this is" / Phase 1 description section and update to reflect Phase 2:

- The "Phase 1 is implemented" paragraph becomes "Phases 1 and 2 are implemented"
- Add to the wired-methods bullet list:
  - `execute_query` accepts the full WHERE/AND/OR/parens grammar with all Firestore filter operators
  - `get_schema_snapshot` populated with inferred foreign-key relationships
  - `explain_query` returns the Firestore query plan
- New pure-logic modules listed: `cache`, `firestore_filter`
- Note that PERMISSION_DENIED/RESOURCE_EXHAUSTED/DEADLINE_EXCEEDED/UNAVAILABLE now produce structured hint messages

Don't touch the user's "Workflow" section at the bottom.

- [ ] **Step 5: Update ROADMAP.md**

In `docs/ROADMAP.md`, update the status snapshot table:

```markdown
| Phase | Status | Spec |
|---|---|---|
| 1 — Read-only MVP | ✅ shipped 2026-05-08 | [...] |
| 2 — Query layer + Map polish | ✅ shipped <today's date> | [`specs/2026-05-08-phase-2-firestore-query-layer-design.md`](specs/2026-05-08-phase-2-firestore-query-layer-design.md) |
| 3 — CRUD | not started | TBD |
| 4 — Multi-DB + Subcollections + Auth UX | not started | TBD |
| 5 — Release & distribution | not started | TBD |
```

Document the smoke-test outcome from Task 11 in a new short note at the bottom of the Phase 2 entry: native JSON kept, or reverted-to-stringified.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/ROADMAP.md
git commit -m "docs: update CLAUDE.md and ROADMAP for Phase 2 shipment"
```

- [ ] **Step 7: Manual smoke tests against `luninora`**

Plug in your real Firestore project. In Tabularis, validate each:
- `WHERE email = 'foo@x.de'` against `customers` returns the matching row
- `WHERE createdAt > '2026-01-01' ORDER BY createdAt DESC LIMIT 50` works (note: timestamps require `TIMESTAMP '...'` cast — `WHERE createdAt > TIMESTAMP '2026-01-01T00:00:00Z'`)
- `(region = 'eu' OR region = 'us') AND active = TRUE` returns the right subset
- The grid footer shows the real total ("1–100 of 5247", not just "100 rows")
- Sequential page-2 → page-3 → page-4 latency is observably constant
- ER-diagram view shows at least one inferred foreign-key edge between collections
- Document the smoke-test outcomes (native JSON for maps and the production validation) in the closeout commit if anything didn't work

This step is manual — there's no automated gate. Document any unexpected behaviour as a new follow-up issue.

---

## Acceptance criteria (mirror of spec § "Acceptance criteria")

Phase 2 is done when **all of these are green**:

- `cargo build --release` succeeds with no warnings
- `cargo clippy --all-targets -- -D warnings` passes
- `cargo test` passes (~70+ unit tests)
- The integration tests in `tests/firestore_emulator.rs` pass against a running Firestore emulator (`cargo test -- --ignored` with `FIRESTORE_EMULATOR_HOST` set)
- Manual end-to-end against `luninora` walked through the spec's "Acceptance criteria" item-5 list
- The native-JSON-for-maps smoke test concluded with a documented decision (kept or reverted)
- `ROADMAP.md` updated: Phase 2 marked as "shipped"
- `CLAUDE.md` updated to describe the Phase 2 implemented state
