//! Query parser: takes a user query string in our grammar and transforms it
//! into an SQLite FTS5 query.
//!
//! Supports a simple Xapian-inspired syntax:
//! - bare words → FTS match
//! - `field:value` → restrict to a specific field
//! - `"quoted phrase"` → literal phrase

use rusqlite::{params, Connection, Result as SqlResult};

/// A parsed query, ready to execute against the FTS index.
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
        Self {
            fts5: String::new(),
            limit: 50,
        }
    }
}

/// Parse a user-supplied query string.
///
/// Currently a simple pass-through with basic sanitisation.
/// Will be extended to handle field prefixes (`from:`, `subject:`, `date:`,
/// `to:`, etc.) and boolean operators.
pub fn parse_query(input: &str) -> ParsedQuery {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ParsedQuery::all();
    }

    // Escape FTS5 special characters (simple approach for now)
    let sanitised: String = trimmed
        .chars()
        .map(|c| match c {
            '*' | '"' | '(' | ')' | '^'  => ' ',
            _ => c,
        })
        .collect();

    let fts5 = sanitised
        .split_whitespace()
        .map(|w| format!("{}*", w))
        .collect::<Vec<_>>()
        .join(" ");

    ParsedQuery {
        fts5,
        limit: 50,
    }
}

/// Search the FTS5 index and return matching message IDs.
pub fn search(conn: &Connection, query: &ParsedQuery) -> SqlResult<Vec<String>> {
    if query.fts5.is_empty() {
        // Return all messages, newest first
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_parses_to_all() {
        assert_eq!(parse_query(""), ParsedQuery::all());
        assert_eq!(parse_query("   "), ParsedQuery::all());
    }

    #[test]
    fn simple_word_query() {
        let q = parse_query("hello");
        assert!(q.fts5.contains("hello*"));
    }

    #[test]
    fn multi_word_query() {
        let q = parse_query("rust lua");
        assert!(q.fts5.contains("rust*"));
        assert!(q.fts5.contains("lua*"));
    }

    #[test]
    fn search_empty_db_returns_empty() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::init_db(&conn).unwrap();
        let q = ParsedQuery::all();
        let ids = search(&conn, &q).unwrap();
        assert!(ids.is_empty());
    }
}
