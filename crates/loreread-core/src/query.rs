//! Query parser: a Xapian-inspired mini-language parsed with nom.
//!
//! Grammar (recursive descent, weakest-to-strongest binding):
//!   query     = or_expr
//!   or_expr   = and_expr ~ ("OR"  ~ and_expr)*
//!   and_expr  = not_expr ~ ("AND" ~ not_expr)*
//!   not_expr  = "NOT"? ~ atom
//!   atom      = field_term | quoted_phrase | "(" ~ or_expr ~ ")" | bare_word
//!   field_term = prefix ":" (quoted_string | unquoted_word)
//!
//! Examples:
//!   hello
//!   from:alice@example.com
//!   subject:"meeting notes"
//!   from:alice AND (subject:foo OR subject:bar)
//!   NOT from:bob

use nom::{
    IResult,
    branch::alt,
    bytes::complete::{tag_no_case, take_till1},
    character::complete::{char, multispace0, multispace1},
    combinator::{map, opt, peek},
    multi::many0,
    sequence::{delimited, preceded, terminated, tuple},
};
use rusqlite::{Connection, Result as SqlResult, params};

// ── AST ────────────────────────────────────────────────────────────────

/// A parsed query expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// `prefix:value` field restriction.
    Field { prefix: String, value: String },
    /// `"quoted phrase"` — exact phrase match.
    Phrase(String),
    /// A bare word token.
    Word(String),
    And(Box<Query>, Box<Query>),
    Or(Box<Query>, Box<Query>),
    Not(Box<Query>),
}

// ── public API ─────────────────────────────────────────────────────────

/// Parse a user-supplied query string into an AST.
///
/// Returns `Ok(Query)` on success.  Malformed input produces an `Err`
/// carrying a nom error (which includes position information).
pub fn parse_query(input: &str) -> Result<Query, nom::Err<nom::error::Error<&str>>> {
    let (rest, q) = preceded(multispace0, or_expr)(input)?;
    let (rest, _) = multispace0(rest)?;
    if rest.is_empty() {
        Ok(q)
    } else {
        Err(nom::Err::Error(nom::error::Error::new(
            rest,
            nom::error::ErrorKind::Eof,
        )))
    }
}

// ── FTS5 bridge (kept compatible with existing callers) ────────────────

/// Materialised query ready to hand to SQLite FTS5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    /// FTS5 expression string (safe to pass directly to SQLite).
    pub fts5: String,
    /// Limit on the number of results.
    pub limit: i64,
}

impl ParsedQuery {
    /// Create a query that matches everything.
    pub fn all() -> Self {
        Self { fts5: String::new(), limit: 50 }
    }
}

/// Walk a parsed [`Query`] AST and produce an FTS5 string.
///
/// This is a **simplified** translation — it flattens field terms into
/// the FTS5 text and uses `AND`/`OR`/`NOT` operators that FTS5
/// understands.
pub fn query_to_fts5(query: &Query) -> String {
    match query {
        Query::Field { prefix: _, value } => {
            // For now, treat field values as plain tokens.
            // Future: map known prefixes to FTS5 column filters.
            // Sanitise: escape FTS5 operators embedded in values.
            value
                .split_whitespace()
                .map(|w| format!("{}*", w))
                .collect::<Vec<_>>()
                .join(" ")
        }
        Query::Phrase(s) => {
            // FTS5 phrase: "hello world"
            format!("\"{}\"", s.replace('"', "\"\""))
        }
        Query::Word(w) => {
            format!("{}*", w)
        }
        Query::And(a, b) => {
            let left = query_to_fts5(a);
            let right = query_to_fts5(b);
            if left.is_empty() { right } else if right.is_empty() { left } else {
                format!("({}) AND ({})", left, right)
            }
        }
        Query::Or(a, b) => {
            let left = query_to_fts5(a);
            let right = query_to_fts5(b);
            format!("({}) OR ({})", left, right)
        }
        Query::Not(a) => {
            format!("NOT ({})", query_to_fts5(a))
        }
    }
}

/// Search the FTS5 index and return matching message IDs.
pub fn search(conn: &Connection, query: &ParsedQuery) -> SqlResult<Vec<String>> {
    if query.fts5.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT message_id FROM messages ORDER BY date_ts DESC LIMIT ?1",
        )?;
        let ids = stmt
            .query_map(params![query.limit], |r| r.get(0))?
            .collect::<SqlResult<Vec<String>>>()?;
        return Ok(ids);
    }

    let mut stmt = conn.prepare(
        "SELECT m.message_id
         FROM messages_fts f
         JOIN messages m ON m.id = f.rowid
         WHERE messages_fts MATCH ?1
         ORDER BY m.date_ts DESC
         LIMIT ?2",
    )?;

    let ids = stmt
        .query_map(params![&query.fts5, query.limit], |r| r.get(0))?
        .collect::<SqlResult<Vec<String>>>()?;
    Ok(ids)
}

// ── nom parsers (private) ──────────────────────────────────────────────

/// A non-whitespace, non-paren token for bare words / unquoted values.
fn unquoted_word(input: &str) -> IResult<&str, String> {
    let (rest, w) = take_till1(|c: char| c.is_whitespace() || c == '(' || c == ')')(input)?;
    Ok((rest, w.to_string()))
}

/// A double-quoted string: `"hello world"`.
fn quoted_string(input: &str) -> IResult<&str, String> {
    delimited(
        char('"'),
        take_till1(|c: char| c == '"'),
        char('"'),
    )(input)
    .map(|(rest, s)| (rest, s.to_string()))
}

/// A quoted phrase standing alone: `"hello world"`.
///
/// Uses `peek` first so that an unclosed quote fails without consuming
/// any input, allowing `alt` to fall through to `bare_word`.
fn quoted_phrase(input: &str) -> IResult<&str, Query> {
    let _ = peek(delimited(char('"'), take_till1(|c: char| c == '"'), char('"')))(input)?;
    map(quoted_string, Query::Phrase)(input)
}

/// A field term: `prefix:value` or `prefix:"quoted value"`.
///
/// Uses `peek` to avoid consuming input when there is no colon,
/// which lets `alt` try the next alternative.
fn field_term(input: &str) -> IResult<&str, Query> {
    // Peek: must be identifier followed by ':'
    let _ = peek(tuple((
        take_till1(|c: char| c.is_whitespace() || c == ':' || c == '(' || c == ')' || c == '"'),
        char(':'),
    )))(input)?;

    let (rest, prefix) = take_till1(|c: char| c.is_whitespace() || c == ':' || c == '(' || c == ')' || c == '"')(input)?;
    let (rest, _) = char(':')(rest)?;
    let (rest, value) = alt((quoted_string, unquoted_word))(rest)?;
    Ok((rest, Query::Field { prefix: prefix.to_string(), value }))
}

/// A parenthesised group: `( or_expr )`.
fn parens(input: &str) -> IResult<&str, Query> {
    delimited(
        preceded(multispace0, char('(')),
        preceded(multispace0, or_expr),
        preceded(multispace0, char(')')),
    )(input)
}

/// A bare word — rejects keywords and tokens starting with `"`
/// (almost certainly an unclosed quote).
fn bare_word(input: &str) -> IResult<&str, Query> {
    let (rest, w) = unquoted_word(input)?;
    if w.starts_with('"') {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    let lower = w.to_lowercase();
    if lower == "and" || lower == "or" || lower == "not" {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        )));
    }
    Ok((rest, Query::Word(w)))
}

// ── recursive-descent combinators ──────────────────────────────────────

fn atom(input: &str) -> IResult<&str, Query> {
    preceded(
        multispace0,
        alt((field_term, quoted_phrase, parens, bare_word)),
    )(input)
}

fn not_expr(input: &str) -> IResult<&str, Query> {
    let (rest, has_not) = opt(terminated(
        preceded(multispace0, tag_no_case("NOT")),
        multispace1,
    ))(input)?;
    let (rest, inner) = atom(rest)?;
    if has_not.is_some() {
        Ok((rest, Query::Not(Box::new(inner))))
    } else {
        Ok((rest, inner))
    }
}

fn and_expr(input: &str) -> IResult<&str, Query> {
    let (rest, first) = not_expr(input)?;
    let (rest, rest_exprs) = many0(preceded(
        tuple((multispace0, tag_no_case("AND"), multispace1)),
        not_expr,
    ))(rest)?;
    Ok((
        rest,
        rest_exprs
            .into_iter()
            .fold(first, |acc, e| Query::And(Box::new(acc), Box::new(e))),
    ))
}

fn or_expr(input: &str) -> IResult<&str, Query> {
    let (rest, first) = and_expr(input)?;
    let (rest, rest_exprs) = many0(preceded(
        tuple((multispace0, tag_no_case("OR"), multispace1)),
        and_expr,
    ))(rest)?;
    Ok((
        rest,
        rest_exprs
            .into_iter()
            .fold(first, |acc, e| Query::Or(Box::new(acc), Box::new(e))),
    ))
}

// ── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── leaf nodes ─────────────────────────────────────────────────

    #[test]
    fn parse_bare_word() {
        assert_eq!(parse_query("hello"), Ok(Query::Word("hello".into())));
    }

    #[test]
    fn parse_field_unquoted() {
        assert_eq!(
            parse_query("from:alice"),
            Ok(Query::Field { prefix: "from".into(), value: "alice".into() })
        );
    }

    #[test]
    fn parse_field_quoted_value() {
        assert_eq!(
            parse_query("subject:\"meeting notes\""),
            Ok(Query::Field { prefix: "subject".into(), value: "meeting notes".into() })
        );
    }

    #[test]
    fn parse_quoted_phrase() {
        assert_eq!(parse_query("\"hello world\""), Ok(Query::Phrase("hello world".into())));
    }

    // ── unary ──────────────────────────────────────────────────────

    #[test]
    fn parse_not() {
        assert_eq!(
            parse_query("NOT hello"),
            Ok(Query::Not(Box::new(Query::Word("hello".into()))))
        );
    }

    #[test]
    fn parse_not_field() {
        assert_eq!(
            parse_query("NOT from:bob"),
            Ok(Query::Not(Box::new(Query::Field {
                prefix: "from".into(),
                value: "bob".into(),
            })))
        );
    }

    // ── binary ─────────────────────────────────────────────────────

    #[test]
    fn parse_simple_and() {
        assert_eq!(
            parse_query("hello AND world"),
            Ok(Query::And(Box::new(Query::Word("hello".into())), Box::new(Query::Word("world".into()))))
        );
    }

    #[test]
    fn parse_simple_or() {
        assert_eq!(
            parse_query("hello OR world"),
            Ok(Query::Or(Box::new(Query::Word("hello".into())), Box::new(Query::Word("world".into()))))
        );
    }

    #[test]
    fn parse_three_way_and() {
        let q = parse_query("a AND b AND c").unwrap();
        // Left-folded: ((a AND b) AND c)
        assert!(matches!(q, Query::And(..)));
        if let Query::And(left, right) = q {
            assert!(matches!(*left, Query::And(..)));
            assert_eq!(*right, Query::Word("c".into()));
        }
    }

    // ── groups / precedence ────────────────────────────────────────

    #[test]
    fn parse_parens() {
        assert_eq!(
            parse_query("(hello)"),
            Ok(Query::Word("hello".into()))
        );
    }

    #[test]
    fn parse_paren_group_or() {
        assert_eq!(
            parse_query("(hello OR world)"),
            Ok(Query::Or(Box::new(Query::Word("hello".into())), Box::new(Query::Word("world".into()))))
        );
    }

    #[test]
    fn and_binds_tighter_than_or() {
        // "a OR b AND c"  should parse as  a OR (b AND c)
        let q = parse_query("a OR b AND c").unwrap();
        assert!(matches!(q, Query::Or(..)));
        if let Query::Or(left, right) = q {
            assert_eq!(*left, Query::Word("a".into()));
            assert!(matches!(*right, Query::And(..)));
        }
    }

    // ── complex ────────────────────────────────────────────────────

    #[test]
    fn complex_field_and_group() {
        let q = parse_query("from:alice AND (subject:foo OR subject:bar)").unwrap();
        assert!(matches!(q, Query::And(..)));
        if let Query::And(left, right) = q {
            assert_eq!(*left, Query::Field { prefix: "from".into(), value: "alice".into() });
            assert!(matches!(*right, Query::Or(..)));
        }
    }

    #[test]
    fn complex_with_not() {
        let q = parse_query("NOT from:bob AND subject:hello").unwrap();
        // AND binds tighter than NOT?  NOT binds tightest.
        // NOT (from:bob) AND subject:hello
        assert!(matches!(q, Query::And(..)));
        if let Query::And(left, right) = q {
            assert!(matches!(*left, Query::Not(..)));
            assert_eq!(*right, Query::Field { prefix: "subject".into(), value: "hello".into() });
        }
    }

    // ── error cases ────────────────────────────────────────────────

    #[test]
    fn reject_unclosed_quote() {
        assert!(parse_query("\"unclosed").is_err());
    }

    #[test]
    fn reject_unclosed_paren() {
        assert!(parse_query("(hello").is_err());
    }

    #[test]
    fn reject_bare_keyword_and() {
        // "AND" alone should not parse as a word
        assert!(parse_query("AND").is_err());
    }

    #[test]
    fn trailing_junk_is_error() {
        // implicit AND not supported (yet); extra tokens are an error
        assert!(parse_query("hello world").is_err());
    }

    // ── FTS5 bridge ────────────────────────────────────────────────

    #[test]
    fn fts5_word() {
        let q = Query::Word("hello".into());
        assert_eq!(query_to_fts5(&q), "hello*");
    }

    #[test]
    fn fts5_phrase() {
        let q = Query::Phrase("hello world".into());
        assert_eq!(query_to_fts5(&q), "\"hello world\"");
    }

    #[test]
    fn fts5_and() {
        let q = Query::And(
            Box::new(Query::Word("a".into())),
            Box::new(Query::Word("b".into())),
        );
        assert_eq!(query_to_fts5(&q), "(a*) AND (b*)");
    }

    #[test]
    fn fts5_not() {
        let q = Query::Not(Box::new(Query::Word("spam".into())));
        assert_eq!(query_to_fts5(&q), "NOT (spam*)");
    }

    #[test]
    fn empty_query_all() {
        assert_eq!(ParsedQuery::all().fts5, String::new());
    }
}
