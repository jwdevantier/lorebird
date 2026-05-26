//! Maildir indexer: walks a maildir, identifies new messages, parses them,
//! and inserts into the SQLite index.
//!
//! Uses `mail_parser` (Stalwart Labs) for parsing and walks the filesystem
//! directly — no `maildir` crate needed.

use rusqlite::{params, Connection, Result as SqlResult};
use std::path::Path;

use crate::message::MailMessage;
use crate::schema;

/// Recursively collect all regular files under `dir`, skipping dotfiles.
fn collect_mail_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            files.extend(collect_mail_files(&path));
        } else if path.is_file() {
            files.push(path);
        }
    }
    files
}

/// Index every message in `maildir_path`, inserting rows into `conn`.
///
/// Skips messages whose `Message-ID` is already present in the database.
///
/// Returns the number of newly inserted messages.
pub fn index_maildir(conn: &Connection, maildir_path: &Path) -> SqlResult<usize> {
    schema::init_db(conn)?;

    let mut inserted = 0usize;

    for subdir in &["cur", "new"] {
        let dir = maildir_path.join(subdir);
        if !dir.is_dir() {
            continue;
        }
        for file_path in collect_mail_files(&dir) {
            let raw = match std::fs::read(&file_path) {
                Ok(b) => b,
                Err(_) => continue,
            };

            let Some(msg) = MailMessage::from_bytes(&raw) else {
                continue;
            };

            let Some(ref msg_id) = msg.message_id else {
                continue;
            };

            // Skip if already indexed
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM messages WHERE message_id = ?1",
                    params![msg_id],
                    |r| r.get(0),
                )
                .unwrap_or(false);

            if exists {
                continue;
            }

            let rel_path = file_path
                .strip_prefix(maildir_path)
                .unwrap_or(&file_path)
                .to_string_lossy()
                .to_string();

            let size: Option<i64> = std::fs::metadata(&file_path)
                .ok()
                .map(|m| m.len() as i64);

            conn.execute(
                "INSERT INTO messages (message_id, subject, from_addr, date_str, date_ts, path, size)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    msg_id,
                    msg.subject,
                    msg.from_addr,
                    msg.date_rfc3339,
                    msg.date_ts,
                    rel_path,
                    size,
                ],
            )?;

            // Insert references (for threading)
            for (seq, ref_id) in msg.references.iter().enumerate() {
                conn.execute(
                    "INSERT OR IGNORE INTO refs (message_id, ref_id, seq) VALUES (?1, ?2, ?3)",
                    params![msg_id, ref_id, seq as i64],
                )?;
            }

            inserted += 1;
        }
    }

    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_empty_maildir_returns_zero() {
        let conn = Connection::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        for sub in &["cur", "new", "tmp"] {
            std::fs::create_dir_all(tmp.path().join(sub)).unwrap();
        }
        let n = index_maildir(&conn, tmp.path()).unwrap();
        assert_eq!(n, 0);
    }
}
