//! CLI tool: run the JWZ threading algorithm on a maildir.
//!
//! Usage:
//!   thread-test --maildir /path/to/maildir
//!
//! Parses every message in the maildir, extracts threading headers, runs
//! the JWZ algorithm, and prints the resulting thread tree.

use clap::Parser;
use loreread_core::message::MailMessage;
use loreread_core::thread::{self, Thread};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "thread-test", about = "Run JWZ threading on a maildir")]
struct Args {
    /// Path to the maildir root
    #[arg(short, long)]
    maildir: PathBuf,

    /// Maximum depth to display (default: 10)
    #[arg(short, long, default_value = "10")]
    depth: usize,

    /// Show per-thread message counts
    #[arg(short, long)]
    verbose: bool,
}

/// Recursively collect all regular files under `dir`, skipping dotfiles.
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

// ── Main ─────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();

    let mut messages: Vec<MailMessage> = Vec::new();
    let mut count = 0usize;
    let mut skipped = 0usize;

    for subdir in &["cur", "new"] {
        let dir = args.maildir.join(subdir);
        if !dir.is_dir() {
            eprintln!("Warning: {}/ not found, skipping", subdir);
            continue;
        }

        for file_path in collect_mail_files(&dir) {
            let raw = match std::fs::read(&file_path) {
                Ok(b) => b,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };

            match MailMessage::from_bytes(&raw) {
                Some(msg) => {
                    messages.push(msg);
                    count += 1;
                }
                None => {
                    skipped += 1;
                }
            }
        }
    }

    eprintln!("Parsed {count} messages (skipped {skipped})");

    // Run threading
    let threads = thread::thread_messages(messages);

    let total_in_tree = count_messages(&threads);
    eprintln!("{} top-level thread(s), {} messages total in tree", threads.len(), total_in_tree);

    for (i, t) in threads.iter().enumerate() {
        let msg_count = count_one(t);
        if args.verbose {
            eprintln!("Thread {}: {} message(s)", i + 1, msg_count);
        }
        println!("\n─── Thread {} ({} msg) ───", i + 1, msg_count);
        print_thread(t, 0, args.depth);
    }
}

fn count_messages(threads: &[Thread<MailMessage>]) -> usize {
    threads.iter().map(count_one).sum()
}

fn count_one<T: loreread_core::thread::Message>(t: &Thread<T>) -> usize {
    let mut n = if t.message.is_some() { 1 } else { 0 };
    for child in &t.children {
        n += count_one(child);
    }
    n
}

fn print_thread(node: &Thread<MailMessage>, indent: usize, max_depth: usize) {
    if indent > max_depth {
        println!("{:indent$}... (max depth)", "", indent = indent * 2);
        return;
    }

    let prefix: String = "  ".repeat(indent);

    match &node.message {
        Some(msg) => {
            let subject = msg.subject.as_deref().unwrap_or("(no subject)");
            let id = msg.message_id.as_deref().unwrap_or("(no message-id)");
            println!("{prefix}● {subject}");
            if indent == 0 {
                println!("{prefix}  id: {id}");
            }
        }
        None => {
            println!("{prefix}◌ (ghost — messages share subject or missing parent)");
        }
    }

    for child in &node.children {
        print_thread(child, indent + 1, max_depth);
    }
}
