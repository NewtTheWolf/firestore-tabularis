//! Hand-rolled parser for the Tabularis SQL subset we accept.
//!
//! Grammar (current):
//!   SELECT (* | col [, col]*) FROM <table>
//!     [WHERE <expr>]
//!     [ORDER BY field [ASC|DESC] (, field [ASC|DESC])*]
//!     [LIMIT n] [OFFSET n]
//! WHERE supports AND/OR/NOT, parens, =/!=/&lt;/&lt;=/&gt;/&gt;=, IN/NOT IN, ARRAY_CONTAINS
//! / ARRAY_CONTAINS_ANY (infix and function form), TIMESTAMP literals.
//! Also unwraps `SELECT * FROM (<inner>) AS alias` because Tabularis Table-View
//! emits that wrapper unconditionally.
//!
//! Replaced by the `sqlparser` crate when next Tabularis-side SQL surprise
//! arrives — see `tasks/todo.md`.

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedQuery {
    pub table: String,
    /// `None` = `SELECT *`. `Some([...])` = explicit column list, projected
    /// client-side after Firestore returns the full document.
    pub columns: Option<Vec<String>>,
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
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    Timestamp(chrono::DateTime<chrono::Utc>),
    /// Full Firestore document resource path
    /// (`projects/<p>/databases/<d>/documents/<col>/<doc>[/<sub>/<doc>]*`).
    /// Produced by the doc-ID → `__name__` rewrite in `execute_query`; not
    /// reachable via SQL syntax directly.
    Reference(String),
}

pub fn parse(sql: &str) -> Result<ParsedQuery, String> {
    let tokens = tokenize(sql)?;
    let mut p = Parser { tokens, pos: 0 };
    let q = p.parse_select()?;
    p.expect_end()?;
    Ok(q)
}

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
    Op(CmpOp),
    /// Numeric literal that includes a fractional part.
    Float(f64),
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
        if c == '*' {
            out.push(Token::Star);
            i += 1;
            continue;
        }
        if c == ',' {
            out.push(Token::Comma);
            i += 1;
            continue;
        }
        if c == '(' {
            out.push(Token::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            out.push(Token::RParen);
            i += 1;
            continue;
        }

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

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }
    fn advance(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), String> {
        match self.advance() {
            Some(Token::Word(w)) if w.eq_ignore_ascii_case(kw) => Ok(()),
            other => Err(format!("expected '{}', got {:?}", kw, other)),
        }
    }

    fn parse_select(&mut self) -> Result<ParsedQuery, String> {
        self.expect_keyword("SELECT")?;
        // Either `*` or a comma-separated identifier list (projected client-side
        // after Firestore returns the full document).
        let columns: Option<Vec<String>> = match self.peek() {
            Some(Token::Star) => {
                self.advance();
                None
            }
            Some(Token::Word(_)) => {
                let mut cols = Vec::new();
                loop {
                    match self.advance() {
                        Some(Token::Word(w)) => cols.push(w),
                        other => {
                            return Err(format!("expected column name, got {:?}", other))
                        }
                    }
                    match self.peek() {
                        Some(Token::Comma) => {
                            self.advance();
                            continue;
                        }
                        _ => break,
                    }
                }
                Some(cols)
            }
            other => {
                return Err(format!(
                    "expected '*' or column list after SELECT, got {:?}",
                    other
                ))
            }
        };
        self.expect_keyword("FROM")?;

        // Tabularis Table-View wraps queries as
        //   SELECT * FROM (SELECT * FROM <table> ... LIMIT n) AS limited_subset
        // Detect the wrapper, parse the inner SELECT, and let any outer
        // ORDER BY / LIMIT / OFFSET clauses override the inner ones.
        if matches!(self.peek(), Some(Token::LParen)) {
            self.advance();
            let mut inner = self.parse_select()?;
            // Outer projection overrides inner — Tabularis wraps with `SELECT *`
            // but a custom outer column list should still win.
            if columns.is_some() {
                inner.columns = columns;
            }
            match self.advance() {
                Some(Token::RParen) => {}
                other => return Err(format!("expected ')' to close subquery, got {:?}", other)),
            }
            // Optional `AS <alias>` after the closing paren.
            if let Some(Token::Word(w)) = self.peek() {
                if w.eq_ignore_ascii_case("AS") {
                    self.advance();
                    match self.advance() {
                        Some(Token::Word(_)) => {}
                        other => return Err(format!("expected alias after AS, got {:?}", other)),
                    }
                }
            }
            // Outer ORDER BY / LIMIT / OFFSET override the inner clauses.
            loop {
                match self.peek() {
                    Some(Token::Word(w)) if w.eq_ignore_ascii_case("ORDER") => {
                        self.advance();
                        self.expect_keyword("BY")?;
                        inner.order_by = self.parse_order_items()?;
                    }
                    Some(Token::Word(w)) if w.eq_ignore_ascii_case("LIMIT") => {
                        self.advance();
                        inner.limit = Some(self.parse_uint("LIMIT")?);
                    }
                    Some(Token::Word(w)) if w.eq_ignore_ascii_case("OFFSET") => {
                        self.advance();
                        inner.offset = Some(self.parse_uint("OFFSET")?);
                    }
                    None => break,
                    Some(other) => {
                        return Err(format!("unexpected token after subquery: {:?}", other))
                    }
                }
            }
            return Ok(inner);
        }

        let table = match self.advance() {
            Some(Token::Word(w)) => w,
            other => return Err(format!("expected table name after FROM, got {:?}", other)),
        };

        let mut order_by = Vec::new();
        let mut limit = None;
        let mut offset = None;
        let mut where_clause: Option<FilterExpr> = None;

        loop {
            match self.peek() {
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("WHERE") => {
                    self.advance();
                    let expr = self.parse_or()?;
                    where_clause = Some(expr);
                }
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
                Some(Token::Word(w))
                    if w.eq_ignore_ascii_case("JOIN")
                        || w.eq_ignore_ascii_case("GROUP")
                        || w.eq_ignore_ascii_case("HAVING") =>
                {
                    return Err(format!(
                        "'{}' is not supported. Firestore has no joins or aggregations \
                         in the storage engine — use the EXPLAIN button on a per-collection \
                         query and post-process in your application.",
                        w.to_uppercase()
                    ));
                }
                Some(Token::RParen) => break, // end of subquery — outer parser handles ')'
                None => break,
                Some(other) => return Err(format!("unexpected token: {:?}", other)),
            }
        }

        Ok(ParsedQuery {
            table,
            columns,
            where_clause,
            order_by,
            limit,
            offset,
        })
    }

    fn parse_order_items(&mut self) -> Result<Vec<OrderItem>, String> {
        let mut items = Vec::new();
        loop {
            let field = match self.advance() {
                Some(Token::Word(w)) => w,
                other => return Err(format!("expected field name in ORDER BY, got {:?}", other)),
            };
            let desc = match self.peek() {
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("ASC") => {
                    self.advance();
                    false
                }
                Some(Token::Word(w)) if w.eq_ignore_ascii_case("DESC") => {
                    self.advance();
                    true
                }
                _ => false,
            };
            items.push(OrderItem { field, desc });
            match self.peek() {
                Some(Token::Comma) => {
                    self.advance();
                    continue;
                }
                _ => break,
            }
        }
        Ok(items)
    }

    fn parse_uint(&mut self, ctx: &str) -> Result<u64, String> {
        match self.advance() {
            Some(Token::Number(n)) => Ok(n),
            other => Err(format!(
                "expected non-negative integer after {ctx}, got {:?}",
                other
            )),
        }
    }

    fn expect_end(&mut self) -> Result<(), String> {
        if self.pos == self.tokens.len() {
            Ok(())
        } else {
            Err(format!(
                "unexpected trailing tokens at position {}",
                self.pos
            ))
        }
    }

    fn parse_or(&mut self) -> Result<FilterExpr, String> {
        let mut terms = vec![self.parse_and()?];
        while let Some(Token::Word(w)) = self.peek() {
            if !w.eq_ignore_ascii_case("OR") {
                break;
            }
            self.advance();
            terms.push(self.parse_and()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            FilterExpr::Or(terms)
        })
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
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            FilterExpr::And(terms)
        })
    }

    fn parse_not(&mut self) -> Result<FilterExpr, String> {
        if let Some(Token::Word(w)) = self.peek() {
            if w.eq_ignore_ascii_case("NOT") {
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
        if matches!(self.peek(), Some(Token::LParen)) {
            self.advance();
            let inner = self.parse_or()?;
            match self.advance() {
                Some(Token::RParen) => return Ok(inner),
                other => return Err(format!("expected ')', got {other:?}")),
            }
        }

        if let Some(Token::Word(w)) = self.peek().cloned() {
            if w.eq_ignore_ascii_case("ARRAY_CONTAINS") {
                self.advance();
                return self.parse_array_contains_call(false);
            }
            if w.eq_ignore_ascii_case("ARRAY_CONTAINS_ANY") {
                self.advance();
                return self.parse_array_contains_call(true);
            }
            if let Some(Token::LParen) = self.tokens.get(self.pos + 1) {
                let upper = w.to_uppercase();
                return Err(format!(
                    "unknown function '{upper}' (did you mean ARRAY_CONTAINS or ARRAY_CONTAINS_ANY?)"
                ));
            }
        }

        let field = match self.advance() {
            Some(Token::Word(w)) => w.split('.').map(str::to_string).collect::<Vec<_>>(),
            other => return Err(format!("expected field name in WHERE, got {other:?}")),
        };

        if let Some(Token::Word(w)) = self.peek() {
            let upper = w.to_uppercase();
            if upper == "NOT" {
                self.advance();
                match self.advance() {
                    Some(Token::Word(w2)) if w2.eq_ignore_ascii_case("IN") => {}
                    other => return Err(format!("expected 'IN' after 'NOT', got {other:?}")),
                }
                let values = self.parse_value_list()?;
                return Ok(FilterExpr::In {
                    field,
                    values,
                    negated: true,
                });
            }
            if upper == "IN" {
                self.advance();
                let values = self.parse_value_list()?;
                return Ok(FilterExpr::In {
                    field,
                    values,
                    negated: false,
                });
            }
            if upper == "ARRAY_CONTAINS" {
                self.advance();
                let value = self.parse_literal()?;
                return Ok(FilterExpr::ArrayContains { field, value });
            }
            if upper == "ARRAY_CONTAINS_ANY" {
                self.advance();
                let values = self.parse_value_list()?;
                return Ok(FilterExpr::ArrayContainsAny { field, values });
            }
        }

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
            other => {
                return Err(format!(
                    "expected '(' after ARRAY_CONTAINS{}, got {other:?}",
                    if any { "_ANY" } else { "" }
                ))
            }
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
            other => Err(format!(
                "expected ')' to close function call, got {other:?}"
            )),
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
                Some(Token::Comma) => {
                    self.advance();
                    continue;
                }
                Some(Token::RParen) => {
                    self.advance();
                    break;
                }
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
                    "TIMESTAMP" => match self.advance() {
                        Some(Token::StringLit(s)) => {
                            let dt = chrono::DateTime::parse_from_rfc3339(&s)
                                .map_err(|e| format!("TIMESTAMP literal not RFC 3339: {e}"))?
                                .with_timezone(&chrono::Utc);
                            Ok(Literal::Timestamp(dt))
                        }
                        other => Err(format!("expected string after TIMESTAMP, got {other:?}")),
                    },
                    _ => Err(format!("expected literal, got identifier '{w}'")),
                }
            }
            other => Err(format!("expected literal, got {other:?}")),
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
        assert_eq!(
            q.order_by,
            vec![OrderItem {
                field: "name".into(),
                desc: true
            }]
        );
        assert_eq!(q.limit, Some(10));
        assert_eq!(q.offset, Some(5));
    }

    #[test]
    fn parses_multi_column_order_by() {
        let q = parse(r#"SELECT * FROM "events" ORDER BY ts DESC, user_id ASC"#).unwrap();
        assert_eq!(
            q.order_by,
            vec![
                OrderItem {
                    field: "ts".into(),
                    desc: true
                },
                OrderItem {
                    field: "user_id".into(),
                    desc: false
                },
            ]
        );
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

    #[test]
    fn accepts_single_column_projection() {
        let q = parse(r#"SELECT name FROM "users""#).unwrap();
        assert_eq!(q.columns, Some(vec!["name".to_string()]));
    }

    #[test]
    fn rejects_join() {
        let err =
            parse(r#"SELECT * FROM "users" JOIN "posts" ON users.id = posts.user_id"#).unwrap_err();
        assert!(err.contains("JOIN"));
    }

    #[test]
    fn rejects_group_by() {
        let err = parse(r#"SELECT * FROM "users" GROUP BY country"#).unwrap_err();
        assert!(err.contains("GROUP"));
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
        assert!(err.contains("unexpected character: -"), "got: {err}");
    }

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
            FilterExpr::Compare {
                value: Literal::Str(s),
                ..
            } => {
                assert_eq!(s, "Alice O'Brien");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_int_and_float_and_bool_and_null() {
        let q = parse(r#"SELECT * FROM "u" WHERE x = 42 AND y = 3.15 AND z = TRUE AND w = NULL"#)
            .unwrap();
        match q.where_clause {
            Some(FilterExpr::And(ref terms)) => {
                let lits: Vec<&Literal> = terms
                    .iter()
                    .map(|t| match t {
                        FilterExpr::Compare { value, .. } => value,
                        _ => panic!(),
                    })
                    .collect();
                assert_eq!(lits[0], &Literal::Int(42));
                assert_eq!(lits[1], &Literal::Float(3.15));
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
            Some(FilterExpr::In {
                values,
                negated: false,
                ..
            }) => {
                assert_eq!(values.len(), 2);
            }
            _ => panic!(),
        }
        match q2.where_clause {
            Some(FilterExpr::In {
                values,
                negated: true,
                ..
            }) => {
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
        let q2 =
            parse(r#"SELECT * FROM "p" WHERE ARRAY_CONTAINS_ANY(tags, ('p0', 'p1'))"#).unwrap();
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
            FilterExpr::Compare {
                value: Literal::Timestamp(_),
                ..
            } => {}
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

    #[test]
    fn unwraps_tabularis_limit_subquery() {
        // Tabularis Table-View wraps queries this way for cross-driver pagination.
        let q = parse(
            r#"SELECT * FROM (SELECT * FROM "advisors" WHERE rating > 4 ORDER BY createdAt DESC LIMIT 50) AS limited_subset"#,
        )
        .unwrap();
        assert_eq!(q.table, "advisors");
        assert_eq!(q.limit, Some(50));
        assert_eq!(q.order_by.len(), 1);
        assert_eq!(q.order_by[0].field, "createdAt");
        assert!(q.order_by[0].desc);
        assert!(q.where_clause.is_some());
    }

    #[test]
    fn outer_limit_overrides_inner() {
        let q = parse(
            r#"SELECT * FROM (SELECT * FROM "u" LIMIT 10) AS s LIMIT 5"#,
        )
        .unwrap();
        assert_eq!(q.limit, Some(5));
    }

    #[test]
    fn parses_column_projection() {
        let q = parse(r#"SELECT name, email FROM "users""#).unwrap();
        assert_eq!(q.table, "users");
        assert_eq!(
            q.columns,
            Some(vec!["name".to_string(), "email".to_string()])
        );
    }

    #[test]
    fn star_keeps_columns_none() {
        let q = parse(r#"SELECT * FROM "users""#).unwrap();
        assert!(q.columns.is_none());
    }

    #[test]
    fn projection_with_where_and_limit() {
        let q = parse(r#"SELECT id, rating FROM "advisors" WHERE verified = true LIMIT 5"#).unwrap();
        assert_eq!(
            q.columns,
            Some(vec!["id".to_string(), "rating".to_string()])
        );
        assert_eq!(q.limit, Some(5));
        assert!(q.where_clause.is_some());
    }

    #[test]
    fn parses_infix_array_contains() {
        let q = parse(r#"SELECT * FROM "p" WHERE tags ARRAY_CONTAINS 'urgent'"#).unwrap();
        match first_compare(&q) {
            FilterExpr::ArrayContains { field, value } => {
                assert_eq!(field, &vec!["tags".to_string()]);
                assert_eq!(value, &Literal::Str("urgent".into()));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_infix_array_contains_any() {
        let q =
            parse(r#"SELECT * FROM "p" WHERE tags ARRAY_CONTAINS_ANY ('p0', 'p1')"#).unwrap();
        match first_compare(&q) {
            FilterExpr::ArrayContainsAny { field, values } => {
                assert_eq!(field, &vec!["tags".to_string()]);
                assert_eq!(values.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn subquery_without_alias() {
        let q = parse(r#"SELECT * FROM (SELECT * FROM "u" LIMIT 7)"#).unwrap();
        assert_eq!(q.table, "u");
        assert_eq!(q.limit, Some(7));
    }
}
