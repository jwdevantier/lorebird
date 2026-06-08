// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2025 Jesper Devantier <jwd@defmacro.it>

//! CLI tool: fetch mail from lore.kernel.org into a maildir or mbox file.
//!
//! Usage mirrors the Go lorefetch program:
//!   lorefetch --query 'search terms' [options]
//!
//! Options:
//!   -q, --query     Xapian search query (required)
//!   -l, --list      Mailing list name (e.g. "linux-kernel")
//!   --maildir        Save as maildir format
//!   --mbox           Save as mbox file
//!   -v, --verbose    Verbosity level (repeat for more: -v, -vv)

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "lorefetch",
    about = "Fetch mail from lore.kernel.org into a maildir or mbox file",
    after_help = "\
Xapian search syntax:
  Lorefetch, like lei, submits queries to the remote public-inbox instance.
  public-inbox servers use Xapian for search.

  Xapian queries use search prefixes with AND, OR, NOT operators and
  parentheses () for grouping.

  Key search prefixes:
    s:           match within Subject
    d:           match date-time range
    b:           match within message body
    t:           match within the To header
    c:           match within the Cc header
    f:           match within the From header
    a:           match within the To, Cc, and From headers
    l:           match contents of the List-Id header
    rt:          match received time

  See https://xapian.org/docs/queryparser.html for full syntax.

Examples:
  * all threads in the last 6 months on qemu-devel with someone in CC/TO:
      lorefetch -q 'l:qemu-devel AND (t:jane@example.org OR f:jane@example.org) AND rt:6.month.ago..now'

  * limit search to linux-kernel list:
      lorefetch -q 'tcp congestion' -l linux-kernel

  * all mail where PATCH is in the subject line of the netdev list:
      lorefetch -q 's:PATCH AND l:netdev'
"
)]
struct Args {
    /// Xapian search query (required)
    #[arg(short, long)]
    query: String,

    /// Mailing list name (e.g. "linux-kernel"); searches /all/ if omitted
    #[arg(short, long)]
    list: Option<String>,

    /// Save as maildir format (creates cur/new/tmp if needed)
    #[arg(long)]
    maildir: Option<PathBuf>,

    /// Save as mbox file
    #[arg(long)]
    mbox: Option<PathBuf>,

    /// Verbosity level (0=quiet, 1=info, 2=debug)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,
}

fn main() {
    let args = Args::parse();

    if args.maildir.is_none() && args.mbox.is_none() {
        eprintln!("Error: specify --maildir or --mbox");
        std::process::exit(1);
    }
    if args.maildir.is_some() && args.mbox.is_some() {
        eprintln!("Error: cannot specify both --maildir and --mbox");
        std::process::exit(1);
    }

    let verbose = args.verbose >= 2;

    if let Some(maildir) = args.maildir {
        match loreread_lorefetch::fetch_to_maildir(
            &args.query,
            args.list.as_deref(),
            &maildir,
            verbose,
        ) {
            Ok(n) => eprintln!("{n} new messages fetched"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    } else if let Some(mbox_path) = args.mbox {
        let client = loreread_lorefetch::LoreClient::new().verbose(verbose);
        match client.fetch_mbox(&args.query, args.list.as_deref()) {
            Ok(content) => {
                if let Err(e) = std::fs::write(&mbox_path, &content) {
                    eprintln!("Write error: {e}");
                    std::process::exit(1);
                }
                eprintln!(
                    "Wrote {} lines to {}",
                    content.lines().count(),
                    mbox_path.display()
                );
            }
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    }
}