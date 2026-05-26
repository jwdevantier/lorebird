//! CLI tool: index a maildir into a SQLite database.
//!
//! Usage:
//!   maildir-index --maildir /path/to/maildir --db /path/to/index.db
//!
//! Walks the maildir, parses every message, and inserts metadata into
//! the SQLite database (creating tables on first run).

use clap::Parser;
use loreread_core::{indexer, schema};
use rusqlite::Connection;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "maildir-index", about = "Index a maildir into a SQLite database")]
struct Args {
    /// Path to the maildir root
    #[arg(short, long)]
    maildir: PathBuf,

    /// Path to the SQLite database file (will be created if it doesn't exist)
    #[arg(short, long)]
    db: PathBuf,
}

fn main() {
    let args = Args::parse();

    let conn = Connection::open(&args.db).unwrap_or_else(|e| {
        eprintln!("Failed to open database {}: {e}", args.db.display());
        std::process::exit(1);
    });

    // Enable WAL mode for concurrent readers + writer
    conn.execute_batch("PRAGMA journal_mode=WAL;").ok();

    schema::init_db(&conn).unwrap_or_else(|e| {
        eprintln!("Failed to initialize schema: {e}");
        std::process::exit(1);
    });

    println!("Indexing maildir: {}", args.maildir.display());
    let start = std::time::Instant::now();

    match indexer::index_maildir(&conn, &args.maildir) {
        Ok(n) => {
            let elapsed = start.elapsed();
            println!("Indexed {n} new messages in {elapsed:.2?}");
            println!(
                "Total messages in DB: {}",
                schema::message_count(&conn).unwrap_or(0)
            );
        }
        Err(e) => {
            eprintln!("Error during indexing: {e}");
            std::process::exit(1);
        }
    }
}
