//! Interactive query parser test tool.
//!
//! Reads query strings from stdin (one per line) and prints the parsed
//! AST and generated FTS5 text.  Type `exit` or Ctrl-D to quit.
//!
//! Examples:
//!   hello
//!   from:alice@example.com
//!   subject:"meeting notes"
//!   from:alice AND (subject:foo OR subject:bar)
//!   NOT from:bob

use std::io::{self, BufRead, Write};

use loreread_core::query::{parse_query, query_to_fts5};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    writeln!(stdout, "Query test tool — type a query and press Enter.  Ctrl-D or 'exit' to quit.\n")
        .unwrap();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
            break;
        }

        match parse_query(trimmed) {
            Ok(ast) => {
                let fts5 = query_to_fts5(&ast);
                writeln!(stdout, "AST:  {:#?}", ast).unwrap();
                writeln!(stdout, "FTS5: {}", fts5).unwrap();
            }
            Err(e) => {
                writeln!(stdout, "ERROR: {}", e).unwrap();
            }
        }
        writeln!(stdout).unwrap();
    }

    writeln!(stdout, "Bye!").unwrap();
}
