//! SQLite schema and migrations for the mail index.
//!
//! Defines tables for message metadata, an FTS5 virtual table for full-text
//! search, and triggers to keep them in sync.

use rusqlite::{Connection, Result as SqlResult};

/// Initialise (or migrate) the mail-index schema in `db`.
///
/// Idempotent — safe to call on an existing database.
pub fn init_db(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "
        -- message metadata
        CREATE TABLE IF NOT EXISTS messages (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            message_id TEXT UNIQUE NOT NULL,
            subject    TEXT,
            from_addr  TEXT,
            date_str   TEXT,
            date_ts    INTEGER,          -- unix epoch, for sorting
            path       TEXT NOT NULL,     -- relative path inside the maildir
            indexed_at INTEGER NOT NULL DEFAULT (unixepoch()),
            size       INTEGER
        );

        -- normalised references for threading (ordered list)
        CREATE TABLE IF NOT EXISTS refs (
            message_id TEXT NOT NULL,
            ref_id     TEXT NOT NULL,
            seq        INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (message_id, ref_id),
            FOREIGN KEY (message_id) REFERENCES messages(message_id)
        );

        -- FTS5 index over subject, from, and body
        CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
            subject,
            from_addr,
            body,
            content=messages,
            content_rowid=id
        );

        -- triggers to keep FTS in sync
        CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
            INSERT INTO messages_fts(rowid, subject, from_addr, body)
            VALUES (new.id, new.subject, new.from_addr, '');
        END;

        CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, body)
            VALUES ('delete', old.id, old.subject, old.from_addr, '');
        END;

        CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
            INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, body)
            VALUES ('delete', old.id, old.subject, old.from_addr, '');
            INSERT INTO messages_fts(rowid, subject, from_addr, body)
            VALUES (new.id, new.subject, new.from_addr, '');
        END;
        ",
    )?;

    Ok(())
}

/// Return the count of messages currently in the database.
pub fn message_count(conn: &Connection) -> SqlResult<i64> {
    conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_db_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        init_db(&conn).unwrap(); // double-init must not fail
    }

    #[test]
    fn message_count_starts_at_zero() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        assert_eq!(message_count(&conn).unwrap(), 0);
    }
}
