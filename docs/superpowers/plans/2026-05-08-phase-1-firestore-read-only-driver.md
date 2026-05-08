# Phase 1 — Read-only Firestore Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the `firestore` Tabularis plugin scaffold into a production-shaped read-only Firestore driver that lists collections, infers per-collection schema from sampled docs, executes `SELECT * … ORDER BY … LIMIT … OFFSET` queries, and surfaces missing-index errors with their console URL.

**Architecture:** Async Tokio dispatch loop on stdio (one JSON-RPC request per line). Plugin-wide settings (project_id, db_id, optional service-account path, optional emulator host) arrive via `initialize`. A lazily-built `FirestoreDb` lives behind `tokio::sync::OnceCell`. Schema inference samples N docs per collection, caches results for the plugin lifetime. A small hand-rolled SQL parser accepts only the supported `SELECT *` grammar; anything else returns a clear error pointing at Phase 2. Centralised `firestore_error::map_error` extracts missing-index URLs and feeds them back via the standard JSON-RPC `error.message` plus structured `error.data.create_index_url`.

**Tech Stack:** Rust stable, `tokio` (multi-thread), `firestore` 0.48, `rustls`, `serde_json`, `serde`, `regex`, `once_cell`. JSON-RPC 2.0 framing on stdio (one request/response per line). Tests via `cargo test`; integration test gated on `FIRESTORE_EMULATOR_HOST`.

**Spec:** [`docs/superpowers/specs/2026-05-08-phase-1-firestore-read-only-driver-design.md`](../specs/2026-05-08-phase-1-firestore-read-only-driver-design.md)

---

## File map

| Path | Disposition | Responsibility |
|---|---|---|
| `Cargo.toml` | modify | Add `serde`, `tokio`, `firestore`, `rustls`, `once_cell`, `regex` |
| `manifest.json` | modify | Flip capabilities, add `settings` array, narrow `data_types` |
| `src/main.rs` | modify | `#[tokio::main]` async loop, declare new modules, drop `#![allow(dead_code)]` |
| `src/rpc.rs` | modify | `async fn handle_line`, extend `error_response` with optional `data`, wire `initialize` |
| `src/error.rs` | unchanged | `PluginError` stays |
| `src/models.rs` | modify | Keep `ConnectionParams` + `inner_params`, add `Settings` struct |
| `src/state.rs` | **create** | `SETTINGS`, `CLIENT`, `SCHEMA_CACHE` globals + `client()` accessor |
| `src/client.rs` | modify | Build `FirestoreDb` from `Settings` (SA path / ADC / emulator) |
| `src/firestore_error.rs` | **create** | `FirestoreError` → JSON-RPC mapping, missing-index URL extraction |
| `src/schema_infer.rs` | **create** | Sample-based column inference (`FieldType` → Tabularis `data_type`) |
| `src/query_parser.rs` | **create** | Hand-rolled parser for `SELECT * FROM "<col>" [ORDER BY …] [LIMIT n] [OFFSET n]` |
| `src/handlers/metadata.rs` | modify | Real `get_databases`, `get_tables`, `get_columns`; rest stay empty |
| `src/handlers/query.rs` | modify | Real `test_connection`, `ping` fast-path, `execute_query` |
| `src/handlers/crud.rs` | unchanged | All `not_implemented` |
| `src/handlers/ddl.rs` | unchanged | All `not_implemented` |
| `tests/firestore_emulator.rs` | **create** | `#[ignore]` integration test that drives the binary against the emulator |
| `CLAUDE.md` | modify | Final pass after implementation lands |

---

## Conventions used in every task

- All `cargo` commands run from the repo root (`/home/newt/Projekte/Personal/NewtTheWolf/firestore-driver`).
- Stage exact file paths in `git add` rather than `git add -A`.
- Commit messages use conventional-style prefixes (`feat:`, `test:`, `chore:`, `refactor:`, `docs:`).
- After each task, the repo must build cleanly: `cargo build` succeeds, `cargo test` passes.
- Don't add features the spec doesn't ask for. If a step looks like it needs more code than shown, re-read the spec section.

---

## Task 1: Update Cargo dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add the runtime + Firestore deps via cargo add**

Run from repo root:

```bash
cargo add serde --features derive
cargo add tokio --features rt-multi-thread,macros,io-std,io-util
cargo add firestore
cargo add rustls
cargo add once_cell
cargo add regex
```

Expected: each command prints an `Adding <crate> v<X.Y.Z> to dependencies` line and updates `Cargo.toml`.

- [ ] **Step 2: Verify the build**

Run: `cargo build`
Expected: succeeds, possibly with a few unused-import warnings from `firestore` since nothing references it yet. No errors.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add tokio, firestore, rustls, once_cell, regex dependencies"
```

---

## Task 2: Update manifest.json

**Files:**
- Modify: `manifest.json`

- [ ] **Step 1: Replace manifest contents**

Overwrite `manifest.json` with:

```json
{
  "$schema": "https://tabularis.dev/schemas/plugin-manifest.json",
  "id": "firestore",
  "name": "Firestore",
  "version": "0.1.0",
  "description": "Tabularis driver plugin for Google Firestore",
  "default_port": null,
  "default_username": "",
  "executable": "firestore-plugin",
  "capabilities": {
    "schemas": false,
    "views": false,
    "routines": false,
    "file_based": false,
    "folder_based": false,
    "no_connection_required": true,
    "identifier_quote": "\"",
    "alter_primary_key": false,
    "alter_column": false,
    "create_foreign_keys": false,
    "manage_tables": false,
    "readonly": true
  },
  "settings": [
    { "key": "project_id", "label": "GCP Project ID", "type": "string", "required": true },
    { "key": "database_id", "label": "Database ID", "type": "string", "default": "(default)" },
    { "key": "service_account_path", "label": "Service Account JSON Path", "type": "string",
      "description": "Optional. If empty, falls back to GOOGLE_APPLICATION_CREDENTIALS or gcloud ADC." },
    { "key": "emulator_host", "label": "Firestore Emulator Host", "type": "string",
      "description": "Optional. e.g. localhost:8080. Overrides production endpoint." },
    { "key": "sample_size", "label": "Schema Inference Sample Size", "type": "number", "default": 50 }
  ],
  "data_types": [
    { "name": "TEXT",      "category": "string",  "requires_length": false, "requires_precision": false },
    { "name": "INTEGER",   "category": "numeric", "requires_length": false, "requires_precision": false },
    { "name": "REAL",      "category": "numeric", "requires_length": false, "requires_precision": false },
    { "name": "BOOLEAN",   "category": "other",   "requires_length": false, "requires_precision": false },
    { "name": "TIMESTAMP", "category": "date",    "requires_length": false, "requires_precision": false }
  ]
}
```

- [ ] **Step 2: Validate JSON syntax**

Run: `python3 -m json.tool manifest.json > /dev/null && echo OK`
Expected: prints `OK`.

- [ ] **Step 3: Commit**

```bash
git add manifest.json
git commit -m "feat(manifest): switch to no_connection_required + settings-driven config"
```

---

## Task 3: Query parser (TDD)

**Files:**
- Create: `src/query_parser.rs`
- Modify: `src/main.rs` (add `mod query_parser;`)

This is a pure function with no Firestore dependency — perfect for tight TDD cycles.

- [ ] **Step 1: Create the empty module and declare it**

Create `src/query_parser.rs`:

```rust
//! Hand-rolled parser for the Phase 1 query grammar:
//!   SELECT * FROM <table> [ORDER BY field [ASC|DESC] (, field [ASC|DESC])*]
//!                         [LIMIT n] [OFFSET n]
//! Anything outside this grammar yields an error — Phase 2 expands the language.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    pub table: String,
    pub order_by: Vec<OrderItem>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderItem {
    pub field: String,
    pub desc: bool,
}

pub fn parse(_sql: &str) -> Result<ParsedQuery, String> {
    Err("not implemented".into())
}

#[cfg(test)]
mod tests {}
```

In `src/main.rs`, add `mod query_parser;` next to the other `mod` declarations.

Run: `cargo build`
Expected: builds with a "function never used" warning for `parse`. No errors.

- [ ] **Step 2: Write the first failing test (simplest happy path)**

Replace the empty `mod tests {}` in `src/query_parser.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_select_star_with_quoted_table() {
        let q = parse(r#"SELECT * FROM "users""#).unwrap();
        assert_eq!(q.table, "users");
        assert_eq!(q.order_by, vec![]);
        assert_eq!(q.limit, None);
        assert_eq!(q.offset, None);
    }
}
```

Run: `cargo test query_parser::tests::parses_select_star_with_quoted_table`
Expected: test FAILS with "not implemented".

- [ ] **Step 3: Implement minimal parser to pass the first test**

Replace the body of `parse` in `src/query_parser.rs` with a tokenizing parser. Replace the entire file contents with:

```rust
//! Hand-rolled parser for the Phase 1 query grammar:
//!   SELECT * FROM <table> [ORDER BY field [ASC|DESC] (, field [ASC|DESC])*]
//!                         [LIMIT n] [OFFSET n]
//! Anything outside this grammar yields an error — Phase 2 expands the language.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    pub table: String,
    pub order_by: Vec<OrderItem>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderItem {
    pub field: String,
    pub desc: bool,
}

pub fn parse(sql: &str) -> Result<ParsedQuery, String> {
    let tokens = tokenize(sql)?;
    let mut p = Parser { tokens, pos: 0 };
    let q = p.parse_select()?;
    p.expect_end()?;
    Ok(q)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),    // identifiers and keywords (lowercased for matching)
    Star,
    Comma,
    Number(u64),
}

fn tokenize(sql: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() { i += 1; continue; }
        if c == '*' { out.push(Token::Star); i += 1; continue; }
        if c == ',' { out.push(Token::Comma); i += 1; continue; }
        if c == '"' || c == '`' {
            let quote = c;
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] as char != quote { i += 1; }
            if i >= bytes.len() {
                return Err(format!("unterminated identifier starting with {quote}"));
            }
            let ident = std::str::from_utf8(&bytes[start..i])
                .map_err(|e| e.to_string())?
                .to_string();
            i += 1; // skip closing quote
            out.push(Token::Word(ident));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_ascii_digit() { i += 1; }
            let n: u64 = std::str::from_utf8(&bytes[start..i])
                .map_err(|e| e.to_string())?
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            out.push(Token::Number(n));
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_ascii_alphanumeric() || ch == '_' { i += 1; } else { break; }
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

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> { self.tokens.get(self.pos) }
    fn advance(&mut self) -> Option<Token> { let t = self.tokens.get(self.pos).cloned(); if t.is_some() { self.pos += 1; } t }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), String> {
        match self.advance() {
            Some(Token::Word(w)) if w.eq_ignore_ascii_case(kw) => Ok(()),
            other => Err(format!("expected '{}', got {:?}", kw, other)),
        }
    }

    fn parse_select(&mut self) -> Result<ParsedQuery, String> {
        self.expect_keyword("SELECT")?;
        match self.advance() {
            Some(Token::Star) => {}
            other => return Err(format!(
                "Phase 1 supports only 'SELECT * FROM \"<collection>\" [ORDER BY field [ASC|DESC], ...] [LIMIT n] [OFFSET n]'. \
                 Non-'*' select lists arrive in Phase 2. (got {:?})", other
            )),
        }
        self.expect_keyword("FROM")?;
        let table = match self.advance() {
            Some(Token::Word(w)) => w,
            other => return Err(format!("expected table name after FROM, got {:?}", other)),
        };

        let mut order_by = Vec::new();
        let mut limit = None;
        let mut offset = None;

        loop {
            match self.peek() {
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("ORDER") => {
                    self.advance();
                    self.expect_keyword("BY")?;
                    order_by = self.parse_order_items()?;
                }
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("LIMIT") => {
                    self.advance();
                    limit = Some(self.parse_uint("LIMIT")?);
                }
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("OFFSET") => {
                    self.advance();
                    offset = Some(self.parse_uint("OFFSET")?);
                }
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("WHERE")
                                     || w.eq_ignore_ascii_case("JOIN")
                                     || w.eq_ignore_ascii_case("GROUP")
                                     || w.eq_ignore_ascii_case("HAVING") => {
                    return Err(format!(
                        "Phase 1 supports only 'SELECT * FROM \"<collection>\" [ORDER BY field [ASC|DESC], ...] [LIMIT n] [OFFSET n]'. \
                         '{}' arrives in Phase 2.", w.to_uppercase()
                    ));
                }
                None => break,
                Some(other) => return Err(format!("unexpected token: {:?}", other)),
            }
        }

        Ok(ParsedQuery { table, order_by, limit, offset })
    }

    fn parse_order_items(&mut self) -> Result<Vec<OrderItem>, String> {
        let mut items = Vec::new();
        loop {
            let field = match self.advance() {
                Some(Token::Word(w)) => w,
                other => return Err(format!("expected field name in ORDER BY, got {:?}", other)),
            };
            let desc = match self.peek() {
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("ASC") => { self.advance(); false }
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("DESC") => { self.advance(); true }
                _ => false,
            };
            items.push(OrderItem { field, desc });
            match self.peek() {
                Some(Token::Comma) => { self.advance(); continue; }
                _ => break,
            }
        }
        Ok(items)
    }

    fn parse_uint(&mut self, ctx: &str) -> Result<u64, String> {
        match self.advance() {
            Some(Token::Number(n)) => Ok(n),
            other => Err(format!("expected non-negative integer after {ctx}, got {:?}", other)),
        }
    }

    fn expect_end(&mut self) -> Result<(), String> {
        if self.pos == self.tokens.len() {
            Ok(())
        } else {
            Err(format!("unexpected trailing tokens at position {}", self.pos))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_select_star_with_quoted_table() {
        let q = parse(r#"SELECT * FROM "users""#).unwrap();
        assert_eq!(q.table, "users");
        assert_eq!(q.order_by, vec![]);
        assert_eq!(q.limit, None);
        assert_eq!(q.offset, None);
    }
}
```

Run: `cargo test query_parser::tests::parses_select_star_with_quoted_table`
Expected: PASS.

- [ ] **Step 4: Add positive coverage tests**

Append the following tests to the `mod tests` block in `src/query_parser.rs`:

```rust
    #[test]
    fn parses_unquoted_table() {
        let q = parse("SELECT * FROM users").unwrap();
        assert_eq!(q.table, "users");
    }

    #[test]
    fn parses_backtick_quoted_table() {
        let q = parse("SELECT * FROM `users`").unwrap();
        assert_eq!(q.table, "users");
    }

    #[test]
    fn keywords_are_case_insensitive() {
        let q = parse(r#"select * from "users" order by name desc limit 10 offset 5"#).unwrap();
        assert_eq!(q.table, "users");
        assert_eq!(q.order_by, vec![OrderItem { field: "name".into(), desc: true }]);
        assert_eq!(q.limit, Some(10));
        assert_eq!(q.offset, Some(5));
    }

    #[test]
    fn parses_multi_column_order_by() {
        let q = parse(r#"SELECT * FROM "events" ORDER BY ts DESC, user_id ASC"#).unwrap();
        assert_eq!(q.order_by, vec![
            OrderItem { field: "ts".into(),      desc: true },
            OrderItem { field: "user_id".into(), desc: false },
        ]);
    }

    #[test]
    fn parses_limit_and_offset_in_either_order() {
        let q = parse(r#"SELECT * FROM "users" OFFSET 100 LIMIT 50"#).unwrap();
        assert_eq!(q.limit, Some(50));
        assert_eq!(q.offset, Some(100));
    }

    #[test]
    fn whitespace_is_flexible() {
        let q = parse("  SELECT\t*\n  FROM \"users\"  ").unwrap();
        assert_eq!(q.table, "users");
    }
```

Run: `cargo test query_parser::tests`
Expected: all pass.

- [ ] **Step 5: Add negative coverage tests (Phase-2 features rejected)**

Append:

```rust
    #[test]
    fn rejects_where_clause() {
        let err = parse(r#"SELECT * FROM "users" WHERE id = 1"#).unwrap_err();
        assert!(err.contains("Phase 2"), "expected Phase 2 message, got: {err}");
        assert!(err.contains("WHERE"));
    }

    #[test]
    fn rejects_non_star_select_list() {
        let err = parse(r#"SELECT name FROM "users""#).unwrap_err();
        assert!(err.contains("Phase 2"), "expected Phase 2 message, got: {err}");
    }

    #[test]
    fn rejects_join() {
        let err = parse(r#"SELECT * FROM "users" JOIN "posts" ON users.id = posts.user_id"#).unwrap_err();
        assert!(err.contains("Phase 2"));
    }

    #[test]
    fn rejects_group_by() {
        let err = parse(r#"SELECT * FROM "users" GROUP BY country"#).unwrap_err();
        assert!(err.contains("Phase 2"));
    }

    #[test]
    fn rejects_missing_from() {
        let err = parse("SELECT *").unwrap_err();
        assert!(err.contains("expected 'FROM'"));
    }

    #[test]
    fn rejects_negative_limit() {
        // Negative numbers don't tokenize as Number(u64); the '-' becomes an unexpected character.
        let err = parse(r#"SELECT * FROM "users" LIMIT -5"#).unwrap_err();
        assert!(err.contains("unexpected character") || err.contains("expected non-negative"));
    }
```

Run: `cargo test query_parser::tests`
Expected: all 13 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/query_parser.rs src/main.rs
git commit -m "feat(query_parser): hand-rolled parser for SELECT * with ORDER BY/LIMIT/OFFSET"
```

---

## Task 4: Schema inference (TDD)

**Files:**
- Create: `src/schema_infer.rs`
- Modify: `src/main.rs` (add `mod schema_infer;`)

The inference logic is pure: in goes a list of "field → set-of-Firestore-types" maps, out comes the column list. The adapter that converts a real `firestore-rs` document into the type map is implemented in Task 10 with `get_columns`. This separation keeps the algorithm independently testable without any Firestore types.

- [ ] **Step 1: Define types and a stub `infer`**

Create `src/schema_infer.rs`:

```rust
//! Sample-based column inference.
//!
//! The classifier (`FieldType`) and the algorithm (`infer`) are decoupled
//! from `firestore-rs`. The handler in `handlers/metadata.rs` is responsible
//! for converting a real `FirestoreDocument` into a `Vec<DocumentTypes>`
//! before calling `infer`.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldType {
    String,
    Integer,
    Double,
    Boolean,
    Timestamp,
    Bytes,
    GeoPoint,
    Reference,
    Array,
    Map,
    Null,
}

/// Map of field-name → set of Firestore types observed for that field within a single document.
/// (A single field within a single document has exactly one type, so the inner Set is conceptual:
/// we use it to fold across all sample docs in `infer`.)
pub type DocumentTypes = BTreeMap<String, FieldType>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
}

impl ColumnInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "data_type": self.data_type,
            "is_nullable": self.is_nullable,
            "column_default": Value::Null,
            "is_primary_key": self.name == "__id__",
            "is_auto_increment": false,
            "comment": if self.name == "__id__" { Value::String("Firestore document ID".into()) } else { Value::Null },
        })
    }
}

pub fn infer(_sample: &[DocumentTypes]) -> Vec<ColumnInfo> {
    Vec::new()
}

#[cfg(test)]
mod tests {}
```

Add `mod schema_infer;` in `src/main.rs`.

Run: `cargo build`
Expected: builds with unused-warning(s); no errors.

- [ ] **Step 2: Write the first failing test (id-only column)**

Replace `mod tests {}` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn doc(pairs: &[(&str, FieldType)]) -> DocumentTypes {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn empty_sample_returns_only_id_column() {
        let cols = infer(&[]);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].name, "__id__");
        assert_eq!(cols[0].data_type, "string");
        assert_eq!(cols[0].is_nullable, false);
    }
}
```

Run: `cargo test schema_infer::tests::empty_sample_returns_only_id_column`
Expected: FAIL (returns empty Vec).

- [ ] **Step 3: Implement minimal `infer`**

Replace the body of `infer` in `src/schema_infer.rs`:

```rust
pub fn infer(sample: &[DocumentTypes]) -> Vec<ColumnInfo> {
    // Always-present synthetic ID column.
    let mut out = vec![ColumnInfo {
        name: "__id__".into(),
        data_type: "string".into(),
        is_nullable: false,
    }];

    // Collect, per field, the set of observed types and whether the field was missing in any doc.
    let mut types_by_field: BTreeMap<String, BTreeSet<FieldType>> = BTreeMap::new();
    let mut missing_in_any: BTreeMap<String, bool> = BTreeMap::new();

    let total = sample.len();
    let mut seen_count: BTreeMap<String, usize> = BTreeMap::new();

    for doc in sample {
        for (k, t) in doc {
            types_by_field.entry(k.clone()).or_default().insert(*t);
            *seen_count.entry(k.clone()).or_insert(0) += 1;
        }
    }

    for (k, count) in &seen_count {
        missing_in_any.insert(k.clone(), *count < total);
    }

    for (name, types) in types_by_field {
        let (data_type, has_null) = classify_set(&types);
        let is_nullable = has_null || *missing_in_any.get(&name).unwrap_or(&false);
        out.push(ColumnInfo { name, data_type, is_nullable });
    }

    out
}

fn classify_set(types: &BTreeSet<FieldType>) -> (String, bool) {
    let has_null = types.contains(&FieldType::Null);
    let mut non_null: Vec<FieldType> = types.iter().copied().filter(|t| *t != FieldType::Null).collect();
    non_null.sort();

    let data_type = match non_null.as_slice() {
        []                            => "null",
        [FieldType::String]           => "string",
        [FieldType::Integer]
        | [FieldType::Double]
        | [FieldType::Integer, FieldType::Double] => "number",
        [FieldType::Boolean]          => "boolean",
        [FieldType::Timestamp]        => "timestamp",
        [FieldType::Bytes]            => "binary",
        [FieldType::GeoPoint]         => "geopoint",
        [FieldType::Reference]        => "reference",
        [FieldType::Array]            => "array",
        [FieldType::Map]              => "map",
        _                             => "mixed",
    };

    (data_type.to_string(), has_null)
}
```

Run: `cargo test schema_infer::tests::empty_sample_returns_only_id_column`
Expected: PASS.

- [ ] **Step 4: Add coverage tests**

Append to the `mod tests` block:

```rust
    #[test]
    fn single_doc_yields_id_plus_alphabetical_fields() {
        let sample = vec![doc(&[("name", FieldType::String), ("age", FieldType::Integer)])];
        let cols = infer(&sample);
        assert_eq!(cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["__id__", "age", "name"]);
        assert_eq!(cols[1].data_type, "number");
        assert_eq!(cols[2].data_type, "string");
        assert!(!cols[1].is_nullable);
    }

    #[test]
    fn integer_and_double_collapse_to_number() {
        let sample = vec![
            doc(&[("score", FieldType::Integer)]),
            doc(&[("score", FieldType::Double)]),
        ];
        let cols = infer(&sample);
        let score = cols.iter().find(|c| c.name == "score").unwrap();
        assert_eq!(score.data_type, "number");
    }

    #[test]
    fn null_co_observed_with_string_yields_nullable_string() {
        let sample = vec![
            doc(&[("note", FieldType::String)]),
            doc(&[("note", FieldType::Null)]),
        ];
        let cols = infer(&sample);
        let note = cols.iter().find(|c| c.name == "note").unwrap();
        assert_eq!(note.data_type, "string");
        assert!(note.is_nullable);
    }

    #[test]
    fn conflicting_types_yield_mixed() {
        let sample = vec![
            doc(&[("flag", FieldType::Boolean)]),
            doc(&[("flag", FieldType::String)]),
        ];
        let cols = infer(&sample);
        let flag = cols.iter().find(|c| c.name == "flag").unwrap();
        assert_eq!(flag.data_type, "mixed");
    }

    #[test]
    fn missing_field_in_some_docs_marks_nullable() {
        let sample = vec![
            doc(&[("name", FieldType::String), ("nickname", FieldType::String)]),
            doc(&[("name", FieldType::String)]),
        ];
        let cols = infer(&sample);
        let name = cols.iter().find(|c| c.name == "name").unwrap();
        let nickname = cols.iter().find(|c| c.name == "nickname").unwrap();
        assert!(!name.is_nullable);
        assert!(nickname.is_nullable);
    }

    #[test]
    fn all_null_yields_null_data_type() {
        let sample = vec![doc(&[("placeholder", FieldType::Null)])];
        let cols = infer(&sample);
        let p = cols.iter().find(|c| c.name == "placeholder").unwrap();
        assert_eq!(p.data_type, "null");
        assert!(p.is_nullable);
    }

    #[test]
    fn nested_map_column_typed_as_map() {
        let sample = vec![doc(&[("address", FieldType::Map)])];
        let cols = infer(&sample);
        let a = cols.iter().find(|c| c.name == "address").unwrap();
        assert_eq!(a.data_type, "map");
    }

    #[test]
    fn id_column_serialises_as_primary_key() {
        let cols = infer(&[]);
        let json = cols[0].to_json();
        assert_eq!(json["is_primary_key"], serde_json::Value::Bool(true));
        assert_eq!(json["comment"], serde_json::Value::String("Firestore document ID".into()));
    }
```

Run: `cargo test schema_infer::tests`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/schema_infer.rs src/main.rs
git commit -m "feat(schema_infer): sample-based column inference with type-conflict resolution"
```

---

## Task 5: Firestore error mapping (TDD)

**Files:**
- Create: `src/firestore_error.rs`
- Modify: `src/main.rs` (add `mod firestore_error;`)

We're not constructing real `FirestoreError` instances in tests (firestore-rs doesn't expose convenient constructors). The mapping logic operates on the message string + an `ErrorKind` we classify separately. Tests drive the message-side function directly; the real classifier is a thin shim that calls into the message-side function from inside the binary.

- [ ] **Step 1: Define types, classifier scaffold, and stub mapper**

Create `src/firestore_error.rs`:

```rust
//! FirestoreError → JSON-RPC error mapping.
//!
//! Phase 1 surfaces missing-index URLs verbatim. Phase 2 will expand this with
//! IAM hints, quota backoff, and retry guidance.

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

/// Coarse classification of a Firestore error, used to pick a response shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    FailedPrecondition,
    Unauthenticated,
    NotFound,
    Other,
}

static INDEX_URL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"https://console\.(?:firebase|cloud)\.google\.com/[^\s'"]+"#).unwrap()
});

pub fn map_message(raw: &str, kind: ErrorKind) -> (i64, String, Option<Value>) {
    if kind == ErrorKind::FailedPrecondition {
        if let Some(m) = INDEX_URL_RE.find(raw) {
            let url = m.as_str().to_string();
            return (
                -32603,
                format!("Missing Firestore index. Create it: {url}"),
                Some(json!({ "create_index_url": url })),
            );
        }
    }
    if kind == ErrorKind::Unauthenticated {
        return (
            -32602,
            format!(
                "Auth failed: {raw}. Set service_account_path in plugin settings or run \
                 'gcloud auth application-default login'."
            ),
            None,
        );
    }
    if kind == ErrorKind::NotFound {
        return (-32602, format!("Not found: {raw}"), None);
    }
    (-32603, format!("Firestore: {raw}"), None)
}

/// Adapter for real `firestore::errors::FirestoreError`. Keeps the public-facing API
/// the handlers call into thin; the `map_message` helper carries the actual logic and
/// is the only thing the unit tests exercise.
pub fn map_error(err: &firestore::errors::FirestoreError) -> (i64, String, Option<Value>) {
    let raw = err.to_string();
    let kind = classify(&raw);
    map_message(&raw, kind)
}

fn classify(raw: &str) -> ErrorKind {
    // Phase 1 uses substring matching on the gRPC status name. firestore-rs surfaces
    // these tokens in the Display output of `FirestoreError::DatabaseError` variants.
    if raw.contains("FAILED_PRECONDITION") { ErrorKind::FailedPrecondition }
    else if raw.contains("UNAUTHENTICATED") { ErrorKind::Unauthenticated }
    else if raw.contains("NOT_FOUND")       { ErrorKind::NotFound }
    else                                    { ErrorKind::Other }
}

#[cfg(test)]
mod tests {}
```

Add `mod firestore_error;` in `src/main.rs`.

Run: `cargo build`
Expected: builds, possibly with dead-code warnings.

- [ ] **Step 2: Add tests covering each branch**

Replace `mod tests {}` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_firebase_console_url() {
        let raw = "FAILED_PRECONDITION: The query requires an index. \
                   You can create it here: https://console.firebase.google.com/v1/r/project/p1/firestore/indexes?create_composite=abc";
        let (code, msg, data) = map_message(raw, ErrorKind::FailedPrecondition);
        assert_eq!(code, -32603);
        assert!(msg.starts_with("Missing Firestore index. Create it: https://console.firebase.google.com/"));
        let url = data.unwrap()["create_index_url"].as_str().unwrap().to_string();
        assert!(url.starts_with("https://console.firebase.google.com/"));
    }

    #[test]
    fn extracts_cloud_console_url() {
        let raw = "FAILED_PRECONDITION: missing index, see https://console.cloud.google.com/firestore/indexes?project=p1";
        let (code, msg, data) = map_message(raw, ErrorKind::FailedPrecondition);
        assert_eq!(code, -32603);
        assert!(msg.contains("https://console.cloud.google.com/"));
        assert!(data.is_some());
    }

    #[test]
    fn failed_precondition_without_url_falls_through() {
        // No index URL in the message → not classified as missing-index; default branch.
        let (code, msg, data) = map_message("FAILED_PRECONDITION: something else", ErrorKind::FailedPrecondition);
        assert_eq!(code, -32603);
        assert!(msg.starts_with("Firestore: "));
        assert!(data.is_none());
    }

    #[test]
    fn unauthenticated_message_includes_setup_hint() {
        let (code, msg, data) = map_message("UNAUTHENTICATED: bad creds", ErrorKind::Unauthenticated);
        assert_eq!(code, -32602);
        assert!(msg.contains("service_account_path"));
        assert!(msg.contains("gcloud auth application-default login"));
        assert!(data.is_none());
    }

    #[test]
    fn not_found_uses_invalid_params_code() {
        let (code, msg, _) = map_message("NOT_FOUND: collection 'gone' missing", ErrorKind::NotFound);
        assert_eq!(code, -32602);
        assert!(msg.starts_with("Not found: "));
    }

    #[test]
    fn other_kind_falls_through_to_internal_error() {
        let (code, msg, data) = map_message("DEADLINE_EXCEEDED", ErrorKind::Other);
        assert_eq!(code, -32603);
        assert!(msg.starts_with("Firestore: "));
        assert!(data.is_none());
    }

    #[test]
    fn url_regex_does_not_match_non_console_links() {
        // Make sure the regex isn't matching unrelated URLs that happen to mention google.com.
        let raw = "FAILED_PRECONDITION: see https://example.com/help and https://google.com/policies";
        let (code, msg, data) = map_message(raw, ErrorKind::FailedPrecondition);
        assert_eq!(code, -32603);
        assert!(msg.starts_with("Firestore: ")); // fell through to default
        assert!(data.is_none());
    }

    #[test]
    fn classifier_recognises_each_status_token() {
        assert_eq!(classify("rpc error: code = FAILED_PRECONDITION"), ErrorKind::FailedPrecondition);
        assert_eq!(classify("rpc error: code = UNAUTHENTICATED"), ErrorKind::Unauthenticated);
        assert_eq!(classify("rpc error: code = NOT_FOUND"), ErrorKind::NotFound);
        assert_eq!(classify("rpc error: code = INTERNAL"), ErrorKind::Other);
    }
}
```

Run: `cargo test firestore_error::tests`
Expected: all 8 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/firestore_error.rs src/main.rs
git commit -m "feat(firestore_error): classify errors and extract missing-index URLs"
```

---

## Task 6: Async runtime + extended error_response

**Files:**
- Modify: `src/main.rs`
- Modify: `src/rpc.rs`

This task converts the dispatch loop to Tokio and extends `error_response` to carry optional structured `data`. Handlers stay sync-returning for now — Task 8 makes them `async`.

- [ ] **Step 1: Convert main.rs to a Tokio loop**

Replace `src/main.rs` entirely with:

```rust
//! Entry point: read JSON-RPC lines from stdin, dispatch, write responses.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

mod client;
mod error;
mod firestore_error;
mod handlers;
mod models;
mod query_parser;
mod rpc;
mod schema_infer;
mod state;
mod utils;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = rpc::handle_line(trimmed).await;
        let mut body = match serde_json::to_string(&response) {
            Ok(s) => s,
            Err(err) => format!(
                "{{\"jsonrpc\":\"2.0\",\"error\":{{\"code\":-32603,\"message\":\"serialization failed: {err}\"}},\"id\":null}}",
            ),
        };
        body.push('\n');
        if stdout.write_all(body.as_bytes()).await.is_err() {
            break;
        }
        let _ = stdout.flush().await;
    }
}
```

Note: `mod state;` is declared here; the file is created in Task 7. `mod query_parser;`, `mod schema_infer;`, `mod firestore_error;` were declared in Tasks 3–5. The crate-level `#![allow(dead_code)]` is intentionally gone — re-add it locally on a struct or module if you hit dead-code warnings during a partial build, but never at crate level.

- [ ] **Step 2: Extend error_response and make dispatch async**

Replace `src/rpc.rs` entirely with:

```rust
//! JSON-RPC dispatch and response helpers.

use serde_json::{json, Value};

use crate::handlers;

pub async fn handle_line(line: &str) -> Value {
    let request: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(err) => return error_response(Value::Null, -32700, &format!("parse error: {err}"), None),
    };

    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    match method.as_str() {
        "initialize" => handlers::query::initialize(id, &params).await,
        "ping" => handlers::query::ping(id, &params).await,
        "test_connection" => handlers::query::test_connection(id, &params).await,

        "get_databases" => handlers::metadata::get_databases(id, &params).await,
        "get_schemas" => handlers::metadata::get_schemas(id, &params),
        "get_tables" => handlers::metadata::get_tables(id, &params).await,
        "get_columns" => handlers::metadata::get_columns(id, &params).await,
        "get_foreign_keys" => handlers::metadata::get_foreign_keys(id, &params),
        "get_indexes" => handlers::metadata::get_indexes(id, &params),
        "get_views" => handlers::metadata::get_views(id, &params),
        "get_view_definition" => handlers::metadata::get_view_definition(id, &params),
        "get_view_columns" => handlers::metadata::get_view_columns(id, &params),
        "get_routines" => handlers::metadata::get_routines(id, &params),
        "get_routine_parameters" => handlers::metadata::get_routine_parameters(id, &params),
        "get_routine_definition" => handlers::metadata::get_routine_definition(id, &params),
        "get_schema_snapshot" => handlers::metadata::get_schema_snapshot(id, &params),
        "get_all_columns_batch" => handlers::metadata::get_all_columns_batch(id, &params),
        "get_all_foreign_keys_batch" => handlers::metadata::get_all_foreign_keys_batch(id, &params),

        "create_view" | "alter_view" | "drop_view" => not_implemented(id, &method),

        "execute_query" => handlers::query::execute_query(id, &params).await,
        "explain_query" => handlers::query::explain_query(id, &params),

        "insert_record" => handlers::crud::insert_record(id, &params),
        "update_record" => handlers::crud::update_record(id, &params),
        "delete_record" => handlers::crud::delete_record(id, &params),

        "get_create_table_sql" => handlers::ddl::get_create_table_sql(id, &params),
        "get_add_column_sql" => handlers::ddl::get_add_column_sql(id, &params),
        "get_alter_column_sql" => handlers::ddl::get_alter_column_sql(id, &params),
        "get_create_index_sql" => handlers::ddl::get_create_index_sql(id, &params),
        "get_create_foreign_key_sql" => handlers::ddl::get_create_foreign_key_sql(id, &params),
        "drop_index" => handlers::ddl::drop_index(id, &params),
        "drop_foreign_key" => handlers::ddl::drop_foreign_key(id, &params),

        other => not_implemented(id, other),
    }
}

pub fn ok_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "result": result,
        "id": id,
    })
}

pub fn error_response(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(d) = data {
        error["data"] = d;
    }
    json!({
        "jsonrpc": "2.0",
        "error": error,
        "id": id,
    })
}

pub fn not_implemented(id: Value, method: &str) -> Value {
    error_response(
        id,
        -32601,
        &format!("method '{method}' is not implemented by this plugin yet"),
        None,
    )
}
```

- [ ] **Step 3: Update existing handler signatures to compile**

This is a temporary alignment so the project keeps building between tasks. We mark the methods that Task 7+ will turn into real implementations as `async` now and have them return the same stub responses.

Replace `src/handlers/query.rs` entirely with:

```rust
//! Connection and query execution.

use serde_json::{json, Value};

use crate::rpc::{not_implemented, ok_response};

pub async fn initialize(id: Value, _params: &Value) -> Value {
    // Filled in Task 7.
    ok_response(id, Value::Null)
}

pub async fn ping(id: Value, _params: &Value) -> Value {
    ok_response(id, Value::Null)
}

pub async fn test_connection(id: Value, _params: &Value) -> Value {
    // Real implementation lands in Task 8.
    ok_response(id, json!({ "success": true }))
}

pub async fn execute_query(id: Value, _params: &Value) -> Value {
    not_implemented(id, "execute_query")
}

pub fn explain_query(id: Value, _params: &Value) -> Value {
    not_implemented(id, "explain_query")
}
```

Replace `src/handlers/metadata.rs` entirely with:

```rust
//! Schema metadata.

use serde_json::{json, Value};

use crate::rpc::ok_response;

pub async fn get_databases(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}

pub fn get_schemas(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}

pub async fn get_tables(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}

pub async fn get_columns(id: Value, _params: &Value) -> Value {
    ok_response(id, json!([]))
}

pub fn get_foreign_keys(id: Value, _params: &Value) -> Value { ok_response(id, json!([])) }
pub fn get_indexes(id: Value, _params: &Value) -> Value { ok_response(id, json!([])) }
pub fn get_views(id: Value, _params: &Value) -> Value { ok_response(id, json!([])) }
pub fn get_view_definition(id: Value, _params: &Value) -> Value { ok_response(id, Value::String(String::new())) }
pub fn get_view_columns(id: Value, _params: &Value) -> Value { ok_response(id, json!([])) }
pub fn get_routines(id: Value, _params: &Value) -> Value { ok_response(id, json!([])) }
pub fn get_routine_parameters(id: Value, _params: &Value) -> Value { ok_response(id, json!([])) }
pub fn get_routine_definition(id: Value, _params: &Value) -> Value { ok_response(id, Value::String(String::new())) }

pub fn get_schema_snapshot(id: Value, _params: &Value) -> Value {
    ok_response(id, json!({ "tables": [], "columns": {}, "foreign_keys": {} }))
}

pub fn get_all_columns_batch(id: Value, _params: &Value) -> Value { ok_response(id, json!({})) }
pub fn get_all_foreign_keys_batch(id: Value, _params: &Value) -> Value { ok_response(id, json!({})) }
```

`src/handlers/crud.rs` and `src/handlers/ddl.rs` are unchanged — they still return `not_implemented`.

- [ ] **Step 4: Create the empty state module so `mod state` resolves**

Create `src/state.rs`:

```rust
//! Globals: SETTINGS, CLIENT, SCHEMA_CACHE.
//! Real contents land in Task 7.
```

- [ ] **Step 5: Verify build + tests still pass**

Run:
```bash
cargo build && cargo test
```
Expected: both succeed. Tests from Tasks 3–5 still pass; nothing new yet.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/rpc.rs src/state.rs src/handlers/query.rs src/handlers/metadata.rs
git commit -m "refactor: switch dispatch loop to tokio, async handlers, structured error data"
```

---

## Task 7: Settings, global state, initialize handler

**Files:**
- Modify: `src/models.rs`
- Modify: `src/state.rs`
- Modify: `src/handlers/query.rs`

- [ ] **Step 1: Add `Settings` struct in models.rs**

Append to `src/models.rs`:

```rust
/// Plugin-wide settings as delivered by the host's `initialize` call.
#[derive(Debug, Clone)]
pub struct Settings {
    pub project_id: String,
    pub database_id: String,
    pub service_account_path: Option<String>,
    pub emulator_host: Option<String>,
    pub sample_size: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            project_id: String::new(),
            database_id: "(default)".into(),
            service_account_path: None,
            emulator_host: None,
            sample_size: 50,
        }
    }
}

impl Settings {
    pub fn from_value(v: &Value) -> Self {
        let mut s = Self::default();
        let Some(obj) = v.as_object() else { return s; };

        if let Some(p) = obj.get("project_id").and_then(Value::as_str) {
            s.project_id = p.to_string();
        }
        if let Some(d) = obj.get("database_id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            s.database_id = d.to_string();
        }
        s.service_account_path = obj.get("service_account_path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        s.emulator_host = obj.get("emulator_host")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(n) = obj.get("sample_size").and_then(Value::as_u64) {
            s.sample_size = n.try_into().unwrap_or(50).max(1);
        }
        s
    }
}
```

- [ ] **Step 2: Wire global state**

Replace `src/state.rs` entirely with:

```rust
//! Global plugin state. All accessors are safe to call from any async context.

use std::collections::HashMap;
use std::sync::RwLock;

use once_cell::sync::{Lazy, OnceCell};

use crate::models::Settings;
use crate::schema_infer::ColumnInfo;

pub static SETTINGS: OnceCell<Settings> = OnceCell::new();
pub static CLIENT: tokio::sync::OnceCell<firestore::FirestoreDb> = tokio::sync::OnceCell::const_new();
pub static SCHEMA_CACHE: Lazy<RwLock<HashMap<String, Vec<ColumnInfo>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

pub fn settings() -> Option<&'static Settings> {
    SETTINGS.get()
}
```

- [ ] **Step 3: Wire the initialize handler**

In `src/handlers/query.rs`, replace the body of `initialize` with:

```rust
pub async fn initialize(id: Value, params: &Value) -> Value {
    let settings_value = params.get("settings").cloned().unwrap_or(Value::Null);
    let settings = crate::models::Settings::from_value(&settings_value);
    let _ = crate::state::SETTINGS.set(settings); // second initialize is a no-op
    ok_response(id, Value::Null)
}
```

- [ ] **Step 4: Add a unit test for Settings::from_value**

Append to `src/models.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_uses_defaults_for_missing_keys() {
        let s = Settings::from_value(&Value::Null);
        assert_eq!(s.project_id, "");
        assert_eq!(s.database_id, "(default)");
        assert!(s.service_account_path.is_none());
        assert!(s.emulator_host.is_none());
        assert_eq!(s.sample_size, 50);
    }

    #[test]
    fn settings_reads_provided_values() {
        let v = json!({
            "project_id": "p1",
            "database_id": "named-db",
            "service_account_path": "/etc/sa.json",
            "emulator_host": "localhost:8080",
            "sample_size": 25
        });
        let s = Settings::from_value(&v);
        assert_eq!(s.project_id, "p1");
        assert_eq!(s.database_id, "named-db");
        assert_eq!(s.service_account_path.as_deref(), Some("/etc/sa.json"));
        assert_eq!(s.emulator_host.as_deref(), Some("localhost:8080"));
        assert_eq!(s.sample_size, 25);
    }

    #[test]
    fn settings_treats_empty_strings_as_unset() {
        let v = json!({ "service_account_path": "", "emulator_host": "" });
        let s = Settings::from_value(&v);
        assert!(s.service_account_path.is_none());
        assert!(s.emulator_host.is_none());
    }

    #[test]
    fn settings_clamps_zero_sample_size_to_one() {
        let v = json!({ "sample_size": 0 });
        let s = Settings::from_value(&v);
        assert_eq!(s.sample_size, 1);
    }
}
```

- [ ] **Step 5: Verify**

```bash
cargo test
```
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/models.rs src/state.rs src/handlers/query.rs
git commit -m "feat(state): plugin-wide Settings, global state, initialize handler"
```

---

## Task 8: Client::connect + test_connection + ping fast path

**Files:**
- Modify: `src/client.rs`
- Modify: `src/handlers/query.rs`

- [ ] **Step 1: Wire FirestoreDb construction**

Replace `src/client.rs` entirely with:

```rust
//! Driver connection layer — builds a `firestore::FirestoreDb` from `Settings`.

use firestore::{FirestoreDb, FirestoreDbOptions};

use crate::error::PluginError;
use crate::models::Settings;

pub async fn build(settings: &Settings) -> Result<FirestoreDb, PluginError> {
    if settings.project_id.is_empty() {
        return Err(PluginError::invalid_params(
            "project_id is empty — set it in plugin settings before connecting",
        ));
    }

    // Honour the standard emulator env var. Setting it here means firestore-rs picks
    // up the emulator endpoint without further config.
    if let Some(host) = &settings.emulator_host {
        // SAFETY: env mutation is fine — single-threaded init (test_connection is the
        // first awaitable that touches this, before any spawned tasks start gRPC work).
        // SAFETY: no other thread reads FIRESTORE_EMULATOR_HOST at this point — Settings
        // is initialised once, and this code runs serially in the dispatch loop.
        unsafe {
            std::env::set_var("FIRESTORE_EMULATOR_HOST", host);
        }
    }

    // If the user supplied an explicit service-account file, point ADC at it via the
    // same env var firestore-rs reads. Empty path is interpreted as "fall back to ADC".
    if let Some(path) = &settings.service_account_path {
        unsafe {
            std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", path);
        }
    }

    let options = FirestoreDbOptions::new(settings.project_id.clone())
        .with_database_id(settings.database_id.clone());

    FirestoreDb::with_options(options)
        .await
        .map_err(|e| PluginError::internal(format!("Firestore connect: {e}")))
}
```

Note: the two `unsafe { std::env::set_var(...) }` blocks are required on Rust 2024 (edition 2024 marks `set_var` as unsafe due to historical thread-safety issues). On the `2021` edition currently in `Cargo.toml`, `set_var` is safe; remove the `unsafe` blocks if `cargo build` complains they're redundant. (Edition pinning is in `Cargo.toml` — keep it 2021 for Phase 1.)

If `FirestoreDbOptions::with_database_id` doesn't exist in the installed `firestore` version (the API moves between minor versions), substitute `FirestoreDbOptions::new(...).with_database(...)` or whichever method the rustdoc surfaces. The unit test in step 3 will surface the wrong signature immediately.

- [ ] **Step 2: Wire test_connection + ping**

In `src/handlers/query.rs`, replace `test_connection` and `ping`:

```rust
pub async fn ping(id: Value, params: &Value) -> Value {
    if crate::state::CLIENT.get().is_some() {
        return ok_response(id, Value::Null);
    }
    test_connection(id, params).await
}

pub async fn test_connection(id: Value, _params: &Value) -> Value {
    let Some(settings) = crate::state::settings() else {
        return crate::rpc::error_response(
            id, -32602,
            "plugin not initialised — host should send 'initialize' before 'test_connection'",
            None,
        );
    };

    let result = crate::state::CLIENT
        .get_or_try_init(|| async { crate::client::build(settings).await.map_err(|e| e.to_string()) })
        .await;

    match result {
        Ok(_db) => {
            // Optional cheap probe goes here in a future task. For now, "we built a client" suffices.
            ok_response(id, json!({ "success": true }))
        }
        Err(msg) => crate::rpc::error_response(id, -32603, &msg, None),
    }
}
```

- [ ] **Step 3: Verify build**

```bash
cargo build
```
Expected: succeeds. If `FirestoreDbOptions::with_database_id` is wrong, fix the call site per the rustdoc, then retry.

- [ ] **Step 4: Verify all existing tests still pass**

```bash
cargo test
```
Expected: passes. (No new test in this task — `test_connection` requires either a real Firestore or the emulator, which the integration test handles in Task 12.)

- [ ] **Step 5: Commit**

```bash
git add src/client.rs src/handlers/query.rs
git commit -m "feat(client): build FirestoreDb from settings + lazy test_connection + ping fast path"
```

---

## Task 9: get_databases and get_tables

**Files:**
- Modify: `src/handlers/metadata.rs`

- [ ] **Step 1: Implement get_databases**

In `src/handlers/metadata.rs`, replace `get_databases`:

```rust
pub async fn get_databases(id: Value, _params: &Value) -> Value {
    let Some(settings) = crate::state::settings() else {
        return crate::rpc::error_response(
            id, -32602, "plugin not initialised", None,
        );
    };
    ok_response(id, json!([settings.database_id]))
}
```

- [ ] **Step 2: Implement get_tables**

Replace `get_tables`:

```rust
pub async fn get_tables(id: Value, _params: &Value) -> Value {
    let db = match ensure_client().await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    use futures::TryStreamExt;
    let names: Vec<String> = match db.list_collection_ids().await {
        Ok(stream) => match stream.try_collect().await {
            Ok(v) => v,
            Err(e) => return error_from(id, &e),
        },
        Err(e) => return error_from(id, &e),
    };

    let mut tables: Vec<Value> = names
        .into_iter()
        .map(|n| json!({ "name": n, "schema": Value::Null, "comment": Value::Null }))
        .collect();
    tables.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    ok_response(id, json!(tables))
}

async fn ensure_client() -> Result<&'static firestore::FirestoreDb, Value> {
    let Some(settings) = crate::state::settings() else {
        return Err(crate::rpc::error_response(
            Value::Null, -32602, "plugin not initialised", None,
        ));
    };
    crate::state::CLIENT
        .get_or_try_init(|| async { crate::client::build(settings).await.map_err(|e| e.to_string()) })
        .await
        .map_err(|msg| crate::rpc::error_response(Value::Null, -32603, &msg, None))
}

fn error_from(id: Value, err: &firestore::errors::FirestoreError) -> Value {
    let (code, msg, data) = crate::firestore_error::map_error(err);
    crate::rpc::error_response(id, code, &msg, data)
}
```

The `id` plumbing through `ensure_client` is awkward — for now we synthesize `Value::Null` IDs in the helper and patch them in the caller below. Refine in Step 3.

- [ ] **Step 3: Patch the id handling**

The previous step's helper drops the request id when it errors. Fix that by inlining the id at the call site. Replace the body of `get_tables` with:

```rust
pub async fn get_tables(id: Value, _params: &Value) -> Value {
    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

    use futures::TryStreamExt;
    let names: Vec<String> = match db.list_collection_ids().await {
        Ok(stream) => match stream.try_collect().await {
            Ok(v) => v,
            Err(e) => return error_from(id, &e),
        },
        Err(e) => return error_from(id, &e),
    };

    let mut tables: Vec<Value> = names
        .into_iter()
        .map(|n| json!({ "name": n, "schema": Value::Null, "comment": Value::Null }))
        .collect();
    tables.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    ok_response(id, json!(tables))
}

async fn resolve_client(id: Value) -> Result<&'static firestore::FirestoreDb, Value> {
    let Some(settings) = crate::state::settings() else {
        return Err(crate::rpc::error_response(
            id, -32602, "plugin not initialised", None,
        ));
    };
    crate::state::CLIENT
        .get_or_try_init(|| async { crate::client::build(settings).await.map_err(|e| e.to_string()) })
        .await
        .map_err(|msg| crate::rpc::error_response(id.clone(), -32603, &msg, None))
}

fn error_from(id: Value, err: &firestore::errors::FirestoreError) -> Value {
    let (code, msg, data) = crate::firestore_error::map_error(err);
    crate::rpc::error_response(id, code, &msg, data)
}
```

Delete the prior `ensure_client` definition.

- [ ] **Step 4: Add `futures` to dependencies**

The `TryStreamExt` import requires `futures`. Run:
```bash
cargo add futures
```
Expected: adds `futures = "0.3.x"` to `Cargo.toml`.

- [ ] **Step 5: Verify build**

```bash
cargo build
```
Expected: succeeds. If `db.list_collection_ids()` returns a different shape in your firestore version (e.g. `Vec<String>` directly rather than a stream), simplify the body accordingly — the goal is `Vec<String>` of root-collection names, alphabetically sorted, wrapped as table objects.

- [ ] **Step 6: Commit**

```bash
git add src/handlers/metadata.rs Cargo.toml Cargo.lock
git commit -m "feat(metadata): real get_databases and get_tables backed by FirestoreDb"
```

---

## Task 10: get_columns with sample-based inference + cache

**Files:**
- Modify: `src/handlers/metadata.rs`
- Modify: `src/schema_infer.rs` (add the firestore-rs adapter)

- [ ] **Step 1: Add a Firestore-document-to-DocumentTypes adapter**

Append to `src/schema_infer.rs` (above `mod tests`):

```rust
/// Convert a single Firestore field value into our coarse `FieldType`.
pub fn classify_value(v: &firestore::FirestoreValue) -> FieldType {
    use firestore::FirestoreValue;
    use firestore_grpc::google::firestore::v1::value::ValueType as V;

    match v.value.value_type.as_ref() {
        Some(V::NullValue(_))      => FieldType::Null,
        Some(V::BooleanValue(_))   => FieldType::Boolean,
        Some(V::IntegerValue(_))   => FieldType::Integer,
        Some(V::DoubleValue(_))    => FieldType::Double,
        Some(V::TimestampValue(_)) => FieldType::Timestamp,
        Some(V::StringValue(_))    => FieldType::String,
        Some(V::BytesValue(_))     => FieldType::Bytes,
        Some(V::ReferenceValue(_)) => FieldType::Reference,
        Some(V::GeoPointValue(_))  => FieldType::GeoPoint,
        Some(V::ArrayValue(_))     => FieldType::Array,
        Some(V::MapValue(_))       => FieldType::Map,
        None                       => FieldType::Null,
    }
}

/// Walk one document's top-level fields and collapse them into a `DocumentTypes` map.
pub fn types_from_document(doc: &firestore::FirestoreDocument) -> DocumentTypes {
    doc.fields
        .iter()
        .map(|(name, val)| (name.clone(), classify_value(val)))
        .collect()
}
```

If the `firestore_grpc` re-export path or `FirestoreValue.value.value_type` field name doesn't match the installed `firestore` version, fix the imports per the rustdoc — the function's contract (return a `FieldType` for any Firestore value) is what matters; the internals are an implementation detail.

- [ ] **Step 2: Implement get_columns**

In `src/handlers/metadata.rs`, replace `get_columns`:

```rust
pub async fn get_columns(id: Value, params: &Value) -> Value {
    let table = params.get("table").and_then(Value::as_str).unwrap_or("").to_string();
    if table.is_empty() {
        return crate::rpc::error_response(id, -32602, "missing 'table' parameter", None);
    }

    if let Some(cached) = crate::state::SCHEMA_CACHE.read().unwrap().get(&table) {
        let cols: Vec<Value> = cached.iter().map(|c| c.to_json()).collect();
        return ok_response(id, json!(cols));
    }

    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    let n = crate::state::settings().map(|s| s.sample_size).unwrap_or(50) as u32;

    let docs: Vec<firestore::FirestoreDocument> = match db
        .fluent()
        .select()
        .from(table.as_str())
        .limit(n)
        .query()
        .await
    {
        Ok(d) => d,
        Err(e) => return error_from(id, &e),
    };

    let sample: Vec<crate::schema_infer::DocumentTypes> = docs
        .iter()
        .map(crate::schema_infer::types_from_document)
        .collect();

    let columns = crate::schema_infer::infer(&sample);
    crate::state::SCHEMA_CACHE
        .write()
        .unwrap()
        .insert(table, columns.clone());

    let json_cols: Vec<Value> = columns.iter().map(|c| c.to_json()).collect();
    ok_response(id, json!(json_cols))
}
```

- [ ] **Step 3: Verify build**

```bash
cargo build
```
Expected: succeeds. Adjust the `db.fluent().select().from(...).limit(n).query()` chain if the installed firestore version uses different method names — the goal is "fetch up to N documents from the named root collection".

- [ ] **Step 4: Verify existing tests still pass**

```bash
cargo test
```
Expected: all unit tests still pass. (No new unit tests for `get_columns` — the Firestore-adapter shape is an integration concern.)

- [ ] **Step 5: Commit**

```bash
git add src/handlers/metadata.rs src/schema_infer.rs
git commit -m "feat(metadata): get_columns with sample inference + per-collection cache"
```

---

## Task 11: execute_query

**Files:**
- Modify: `src/handlers/query.rs`

- [ ] **Step 1: Implement execute_query**

In `src/handlers/query.rs`, replace `execute_query` and add a row serialiser:

```rust
pub async fn execute_query(id: Value, params: &Value) -> Value {
    let sql = params.get("query").and_then(Value::as_str).unwrap_or("").to_string();
    let parsed = match crate::query_parser::parse(&sql) {
        Ok(p) => p,
        Err(e) => return crate::rpc::error_response(id, -32602, &e, None),
    };

    let db = match resolve_client(id.clone()).await {
        Ok(db) => db,
        Err(resp) => return resp,
    };

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
    if !order_items.is_empty() {
        q = q.order_by(order_items);
    }
    if let Some(n) = parsed.limit { q = q.limit(n as u32); }
    if let Some(o) = parsed.offset { q = q.offset(o as u32); }

    let started = std::time::Instant::now();
    let docs: Vec<firestore::FirestoreDocument> = match q.query().await {
        Ok(d) => d,
        Err(e) => return error_from_query(id, &e),
    };
    let elapsed = started.elapsed().as_millis() as u64;

    let columns = match crate::state::SCHEMA_CACHE.read().unwrap().get(&parsed.table) {
        Some(c) => c.clone(),
        None => {
            // Infer on the fly (caller will hit the cache next time via get_columns).
            let sample: Vec<_> = docs.iter().map(crate::schema_infer::types_from_document).collect();
            crate::schema_infer::infer(&sample)
        }
    };

    let column_names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let rows: Vec<Value> = docs.iter().map(|d| serialize_row(d, &columns)).collect();

    ok_response(id, json!({
        "columns": column_names,
        "rows": rows,
        "total_count": rows.len(),
        "execution_time_ms": elapsed,
    }))
}

fn serialize_row(doc: &firestore::FirestoreDocument, columns: &[crate::schema_infer::ColumnInfo]) -> Value {
    let id = doc_short_id(doc);
    let mut row: Vec<Value> = Vec::with_capacity(columns.len());
    for col in columns {
        if col.name == "__id__" {
            row.push(Value::String(id.clone()));
            continue;
        }
        match doc.fields.get(&col.name) {
            Some(v) => row.push(serialize_value(v)),
            None => row.push(Value::Null),
        }
    }
    Value::Array(row)
}

/// Last path segment of a doc's resource name — the human-friendly document ID.
fn doc_short_id(doc: &firestore::FirestoreDocument) -> String {
    doc.name.rsplit('/').next().unwrap_or("").to_string()
}

fn serialize_value(v: &firestore::FirestoreValue) -> Value {
    use firestore_grpc::google::firestore::v1::value::ValueType as V;
    match v.value.value_type.as_ref() {
        Some(V::NullValue(_)) | None => Value::Null,
        Some(V::BooleanValue(b))   => Value::Bool(*b),
        Some(V::IntegerValue(n))   => json!(n),
        Some(V::DoubleValue(f))    => json!(f),
        Some(V::StringValue(s))    => Value::String(s.clone()),
        Some(V::BytesValue(b))     => {
            use base64::Engine;
            Value::String(base64::engine::general_purpose::STANDARD.encode(b))
        }
        Some(V::TimestampValue(t)) => {
            // RFC 3339 via chrono. The `nanos` field on a Firestore Timestamp is i32 in [0, 1e9).
            let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(t.seconds, t.nanos as u32);
            match dt {
                Some(d) => Value::String(d.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
                None    => Value::Null,
            }
        }
        Some(V::ReferenceValue(r)) => Value::String(r.clone()),
        Some(V::GeoPointValue(g))  => json!({ "lat": g.latitude, "lng": g.longitude }),
        Some(V::ArrayValue(a))     => {
            let items: Vec<Value> = a.values.iter().map(|x| {
                serialize_value(&firestore::FirestoreValue { value: x.clone() })
            }).collect();
            Value::String(serde_json::to_string(&items).unwrap_or_default())
        }
        Some(V::MapValue(m))       => {
            let map: serde_json::Map<String, Value> = m.fields.iter().map(|(k, x)| {
                (k.clone(), serialize_value(&firestore::FirestoreValue { value: x.clone() }))
            }).collect();
            Value::String(serde_json::to_string(&Value::Object(map)).unwrap_or_default())
        }
    }
}

fn error_from_query(id: Value, err: &firestore::errors::FirestoreError) -> Value {
    let (code, msg, data) = crate::firestore_error::map_error(err);
    crate::rpc::error_response(id, code, &msg, data)
}

async fn resolve_client(id: Value) -> Result<&'static firestore::FirestoreDb, Value> {
    let Some(settings) = crate::state::settings() else {
        return Err(crate::rpc::error_response(id, -32602, "plugin not initialised", None));
    };
    crate::state::CLIENT
        .get_or_try_init(|| async { crate::client::build(settings).await.map_err(|e| e.to_string()) })
        .await
        .map_err(|msg| crate::rpc::error_response(id.clone(), -32603, &msg, None))
}
```

The `resolve_client` helper is duplicated in `metadata.rs` and `query.rs` — leave the duplication for Phase 1 (DRY refactor lands later when there's a third caller). Same for `error_from`. The duplication is intentional, not an oversight.

- [ ] **Step 2: Add base64 + chrono dependencies**

```bash
cargo add base64
cargo add chrono --no-default-features --features clock,std
```
Expected: adds `base64 = "0.22.x"` and `chrono = "0.4.x"`. The `--no-default-features --features clock,std` keeps chrono trim — we only need `DateTime::<Utc>::from_timestamp` and `to_rfc3339_opts`.

- [ ] **Step 3: Verify build + tests**

```bash
cargo build && cargo test
```
Expected: both succeed. Adjust the `firestore_grpc` import path or the timestamp formatting if your firestore version differs — the contract is "every Firestore value becomes a JSON value following the spec table".

- [ ] **Step 4: Commit**

```bash
git add src/handlers/query.rs Cargo.toml Cargo.lock
git commit -m "feat(query): execute_query with order_by/limit/offset + row serialisation"
```

---

## Task 12: Integration test (emulator-gated)

**Files:**
- Create: `tests/firestore_emulator.rs`

- [ ] **Step 1: Write the integration test**

Create `tests/firestore_emulator.rs`:

```rust
//! End-to-end: spawn the plugin binary as a child process and drive it over stdio.
//!
//! Skipped unless `FIRESTORE_EMULATOR_HOST` is set. CI starts the emulator
//! and seeds a small fixture before running `cargo test --ignored`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Plugin {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
}

impl Plugin {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_firestore-plugin"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn plugin");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        Plugin { child, stdin, out: BufReader::new(stdout) }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let line = format!("{}\n", req);
        self.stdin.write_all(line.as_bytes()).expect("write");
        self.stdin.flush().expect("flush");

        let mut buf = String::new();
        self.out.read_line(&mut buf).expect("read");
        serde_json::from_str(&buf).expect("parse response")
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn emulator_host() -> Option<String> {
    std::env::var("FIRESTORE_EMULATOR_HOST").ok()
}

#[test]
#[ignore]
fn end_to_end_against_emulator() {
    let host = emulator_host().expect("FIRESTORE_EMULATOR_HOST not set — set it (e.g. localhost:8080) and seed the fixture");
    let project = std::env::var("FIRESTORE_TEST_PROJECT").unwrap_or_else(|_| "demo-project".to_string());

    let mut p = Plugin::spawn();

    let init = p.call("initialize", json!({
        "settings": { "project_id": project, "emulator_host": host, "sample_size": 50 }
    }));
    assert!(init.get("error").is_none(), "initialize failed: {init}");

    let test = p.call("test_connection", json!({ "params": {} }));
    assert_eq!(test["result"]["success"], Value::Bool(true), "test_connection: {test}");

    let dbs = p.call("get_databases", json!({ "params": {} }));
    assert!(dbs["result"].is_array(), "get_databases: {dbs}");

    let tables = p.call("get_tables", json!({ "params": {} }));
    assert!(tables["result"].is_array(), "get_tables: {tables}");
}
```

- [ ] **Step 2: Verify the test is skipped without the env var**

```bash
cargo test --test firestore_emulator
```
Expected: `1 ignored` line — the `#[ignore]` attribute keeps the test out of the default run.

- [ ] **Step 3: (Optional) Run against an emulator locally**

If you have the Firestore emulator running:
```bash
FIRESTORE_EMULATOR_HOST=localhost:8080 cargo test --test firestore_emulator -- --ignored
```
Expected: passes. Errors usually mean the seeded data is missing or the emulator host string is wrong.

- [ ] **Step 4: Commit**

```bash
git add tests/firestore_emulator.rs
git commit -m "test(integration): end-to-end emulator-gated subprocess test"
```

---

## Task 13: Final verification + CLAUDE.md update

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Lint clean**

```bash
cargo clippy --all-targets -- -D warnings
```
Expected: no warnings. Fix anything clippy flags inline; common Phase 1 issues are unused imports left over from the scaffold and `Vec::new()` calls that could be `vec![]`.

- [ ] **Step 2: Format**

```bash
cargo fmt --all -- --check
```
If it reports diffs, run `cargo fmt --all` and re-commit the formatting changes separately.

- [ ] **Step 3: Verify the full test suite**

```bash
cargo test
```
Expected: all unit tests pass; integration test reported as `ignored`.

- [ ] **Step 4: Refresh CLAUDE.md to reflect the implemented state**

Open `CLAUDE.md` and replace the "What this is" paragraph (currently describing the scaffold) and the "Driver layer (the empty seat)" subsection with text that reflects reality post-Phase-1: `client.rs` builds a `FirestoreDb`, `state.rs` holds the globals, `schema_infer.rs` and `query_parser.rs` exist, `firestore_error.rs` extracts missing-index URLs, the dispatch loop is async-Tokio. Keep the same overall structure (Tabularis protocol, taxonomy mapping, etc.) — only the "what's wired" facts change.

The Workflow section at the bottom stays exactly as the user added it.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md to reflect Phase 1 implementation"
```

- [ ] **Step 6: Smoke-test via dev-install**

```bash
just dev-install
```
Expected: builds the binary, copies it + `manifest.json` into the Tabularis plugin folder. Open Tabularis, verify in Settings → Plugins → Firestore that the settings form shows the five fields, fill them in, create a connection, click into a collection and confirm rows appear.

This step is manual — there's no automated gate. Document any unexpected behavior as a new task in `tasks/todo.md` per the repo's workflow.

---

## Acceptance criteria (mirror of spec §"Acceptance criteria")

Phase 1 is done when **all of these are green**:

- `cargo build --release` succeeds with no warnings
- `cargo clippy --all-targets -- -D warnings` passes
- `cargo test` passes (all unit tests)
- The integration test in `tests/firestore_emulator.rs` passes against a running Firestore emulator (`cargo test -- --ignored`)
- Manual smoke test against a real Firestore project shows: connection works, collections appear, columns inferred, sorting + paginating works in the data grid, missing-index error surfaces a clickable console URL
- CLAUDE.md is updated to describe the implemented state, not the scaffold
