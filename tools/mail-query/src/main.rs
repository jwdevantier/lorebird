//! CLI tool: query the mail index and display results as threads.
//!
//! Usage:
//!   mail-query --db <path> --maildir <path> [--msgs-only] [--limit N] <query…>
//!
//! Builds the JWZ thread tree from the maildir, executes the search query
//! against the SQLite FTS index, and prints the threads containing matches.
//! With `--msgs-only`, prints matching message-IDs and subjects in a table
//! (no threading).

use clap::Parser;
use loreread_core::message::MailMessage;
use loreread_core::query::{self, ParsedQuery};
use loreread_core::schema;
use loreread_core::thread::{self, Thread};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mail-query", about = "Search mail with JWZ threading")]
struct Args {
    /// Path to the SQLite database file
    #[arg(short, long)]
    db: PathBuf,

    /// Path to the maildir root
    #[arg(short, long)]
    maildir: PathBuf,

    /// Print matching message-IDs and subjects only (no threading)
    #[arg(long)]
    msgs_only: bool,

    /// Maximum number of results (default: 50)
    #[arg(short, long, default_value = "50")]
    limit: i64,

    /// Search query (Xapian-style: words, prefixes, AND/OR/NOT, date: ranges)
    #[arg(trailing_var_arg = true, required = true)]
    query: Vec<String>,
}

// ── maildir walking (shared with thread-test) ──────────────────────────

fn collect_mail_files(dir: &std::path::Path) -> Vec<PathBuf> {
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

// ── thread printing ────────────────────────────────────────────────────

fn count_one<T: thread::Message>(t: &Thread<T>) -> usize {
    let mut n = if t.message.is_some() { 1 } else { 0 };
    for child in &t.children {
        n += count_one(child);
    }
    n
}

fn print_thread(node: &Thread<MailMessage>, indent: usize, max_depth: usize, matches: &std::collections::HashSet<String>) {
    if indent > max_depth {
        println!("{:indent$}... (max depth)", "", indent = indent * 2);
        return;
    }

    let prefix = "  ".repeat(indent);

    match &node.message {
        Some(msg) => {
            let subject = msg.subject.as_deref().unwrap_or("(no subject)");
            let id = msg.message_id.as_deref().unwrap_or("(no message-id)");
            if matches.contains(id) {
                println!("{prefix}● {subject}");
            } else {
                println!("{prefix}○ {subject}");
            }
            if indent == 0 {
                println!("{prefix}  id: {id}");
            }
        }
        None => {
            println!("{prefix}◌ (ghost — messages share subject or missing parent)");
        }
    }

    for child in &node.children {
        print_thread(child, indent + 1, max_depth, matches);
    }
}

// ── main ───────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();

    // ── open DB ──
    let conn = Connection::open(&args.db).unwrap_or_else(|e| {
        eprintln!("Failed to open database {}: {e}", args.db.display());
        std::process::exit(1);
    });
    conn.execute_batch("PRAGMA journal_mode=WAL;").ok();
    schema::init_db(&conn).unwrap_or_else(|e| {
        eprintln!("Failed to init schema: {e}");
        std::process::exit(1);
    });

    // ── parse query ──
    let query_str = args.query.join(" ");
    let ast = match query::parse_query(&query_str) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("Query parse error: {e}");
            std::process::exit(1);
        }
    };
    let pq = ParsedQuery::from_ast(&ast, args.limit);

    // ── search DB ──
    let search_start = std::time::Instant::now();
    let matched_ids: Vec<String> = match query::search(&conn, &pq) {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("Search error: {e}");
            std::process::exit(1);
        }
    };
    let search_elapsed = search_start.elapsed();
    eprintln!("Query returned {} result(s) in {search_elapsed:.2?}", matched_ids.len());

    if args.msgs_only {
        print_msgs_only(&conn, &matched_ids);
        return;
    }

    // ── build threads from maildir ──
    let thread_start = std::time::Instant::now();
    let (threads, thread_index) = build_threads(&args.maildir);
    let thread_elapsed = thread_start.elapsed();
    eprintln!(
        "Built {} top-level thread(s) in {thread_elapsed:.2?}",
        threads.len()
    );

    // ── filter threads by query matches ──
    let match_set: std::collections::HashSet<String> = matched_ids.iter().cloned().collect();
    let mut seen_threads: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for id in &matched_ids {
        if let Some(&ndx) = thread_index.get(id) {
            seen_threads.insert(ndx);
        }
    }

    eprintln!("Matches across {} thread(s)", seen_threads.len());

    for thread_ndx in seen_threads {
        let t = &threads[thread_ndx];
        let msg_count = count_one(t);
        println!("\n─── Thread {} ({} msg) ───", thread_ndx + 1, msg_count);
        print_thread(t, 0, 10, &match_set);
    }
}

// ── helpers ────────────────────────────────────────────────────────────

fn build_threads(maildir: &PathBuf) -> (Vec<Thread<MailMessage>>, HashMap<String, usize>) {
    let mut messages: Vec<MailMessage> = Vec::new();

    for subdir in &["cur", "new"] {
        let dir = maildir.join(subdir);
        if !dir.is_dir() {
            continue;
        }
        for file_path in collect_mail_files(&dir) {
            let raw = match std::fs::read(&file_path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if let Some(msg) = MailMessage::from_bytes(&raw) {
                messages.push(msg);
            }
        }
    }

    let threads = thread::thread_messages(messages);
    let index = thread::build_thread_index(&threads);
    (threads, index)
}

fn print_msgs_only(conn: &Connection, ids: &[String]) {
    // Print header
    println!("{:<50}  {}", "MESSAGE-ID", "SUBJECT");
    println!("{:-<50}  {:-<50}", "", "");

    for id in ids {
        let subject: String = conn
            .query_row(
                "SELECT COALESCE(subject, '(no subject)') FROM mail_ndx WHERE message_id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "(not in DB)".into());

        println!("{:<50}  {}", id, subject);
    }
}
