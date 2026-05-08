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
    Word(String), // identifiers and keywords; keywords matched case-insensitively at parse time
    Star,
    Comma,
    Number(u64),
    Symbol(char), // single characters not otherwise classified (=, ., etc.)
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
            i += 1; // skip closing quote
            out.push(Token::Word(ident));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_ascii_digit() {
                i += 1;
            }
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
                if ch.is_ascii_alphanumeric() || ch == '_' {
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
        // Tokenize any other printable character as a Symbol so the parser
        // can produce a meaningful error (e.g. Phase 2 rejection) rather than
        // failing silently in the tokenizer.
        out.push(Token::Symbol(c));
        i += 1;
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
                Some(Token::Word(w))
                    if w.eq_ignore_ascii_case("WHERE")
                        || w.eq_ignore_ascii_case("JOIN")
                        || w.eq_ignore_ascii_case("GROUP")
                        || w.eq_ignore_ascii_case("HAVING") =>
                {
                    return Err(format!(
                        "Phase 1 supports only 'SELECT * FROM \"<collection>\" [ORDER BY field [ASC|DESC], ...] [LIMIT n] [OFFSET n]'. \
                         '{}' arrives in Phase 2.", w.to_uppercase()
                    ));
                }
                None => break,
                Some(other) => return Err(format!("unexpected token: {:?}", other)),
            }
        }

        Ok(ParsedQuery {
            table,
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
    fn rejects_where_clause() {
        let err = parse(r#"SELECT * FROM "users" WHERE id = 1"#).unwrap_err();
        assert!(
            err.contains("Phase 2"),
            "expected Phase 2 message, got: {err}"
        );
        assert!(err.contains("WHERE"));
    }

    #[test]
    fn rejects_non_star_select_list() {
        let err = parse(r#"SELECT name FROM "users""#).unwrap_err();
        assert!(
            err.contains("Phase 2"),
            "expected Phase 2 message, got: {err}"
        );
    }

    #[test]
    fn rejects_join() {
        let err =
            parse(r#"SELECT * FROM "users" JOIN "posts" ON users.id = posts.user_id"#).unwrap_err();
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
        assert!(err.contains("expected non-negative"), "got: {err}");
    }
}
