# lorebird

**Index and browse a maildir with GTK, Lua, and SQLite.**

A desktop application built with Rust and GTK4, embedding a Lua runtime
for extensibility, backed by SQLite with full-text search (FTS5).

## Quick start

```bash
# Build the release binary
nix build

# Run it
./result/bin/lorebird

# Or enter the dev shell and cargo-run
nix develop
cargo run -p lorebird
```

## Workspace structure

```
lorebird/
├── Cargo.toml                 # workspace manifest
├── Cargo.lock
├── flake.nix
├── crates/
│   ├── lorebird-core/         # email logic: threading, schema, indexing, query
│   │   └── src/
│   │       ├── thread.rs      # JWZ threading algorithm
│   │       ├── message.rs     # mail_parser → domain types bridge
│   │       ├── schema.rs      # SQLite FTS5 tables & migrations
│   │       ├── indexer.rs     # maildir → SQLite indexer
│   │       └── query.rs       # FTS5 query parser & search
│   ├── lorebird-lua/          # Lua VM integration (mlua)
│   └── lorebird-gtk/          # GTK4 UI (binary: lorebird)
└── tools/
    ├── maildir-index/         # CLI: index a maildir into SQLite
    └── thread-test/           # CLI: run JWZ threading on a maildir
```

## Development

### Enter the dev shell

```bash
nix develop
```

This drops you into a shell with:

| Tool | Purpose |
|---|---|
| `cargo`, `rustc` | Rust compiler & build tool |
| `rust-analyzer` | LSP server (IDE integration) |
| `clippy` | Rust linter |
| `rustfmt` | Rust formatter |
| `pkg-config` | Locate system libraries |
| `gtk4` (dev) | GTK4 headers & libraries for gtk-rs |
| `sqlite3` | SQLite CLI (`sqlite-interactive`) |

### Fast iteration (no GTK)

Most logic lives in `lorebird-core` which has **zero GUI dependencies** —
builds in ~0.2s instead of ~45s.

```bash
# Check core for errors (fast)
cargo check -p lorebird-core

# Run all core tests (36 tests, <0.1s)
cargo test -p lorebird-core

# Run only thread tests (26 tests)
cargo test -p lorebird-core thread::

# Run a single test
cargo test -p lorebird-core thread::tests::linear_thread_by_references
```

### Edit-Compile-Run loop

```bash
nix develop               # enter shell once

cargo check --workspace   # check everything (fast)
cargo build --workspace   # debug build all
cargo run -p lorebird     # build + run the GTK app
cargo test --workspace    # run all tests

# Release build
cargo build --release
```

### CLI tools

Two standalone CLI binaries for testing functionality without the GUI:

```bash
# Build a tool
cargo build -p thread-test
cargo build -p maildir-index

# Run (pass args after --)
cargo run -p thread-test -- --maildir /path/to/maildir
cargo run -p maildir-index -- --maildir /path/to/maildir --db /tmp/index.db

# Or run the built binary directly
./target/debug/thread-test --maildir ~/Mail/lkml --depth 5
./target/debug/maildir-index --maildir ~/Mail/lkml --db ~/Mail/index.db

# Release builds
cargo build --release -p thread-test
./target/release/thread-test --maildir ~/Mail/lkml
```

### IDE setup

`rust-analyzer` is available in the dev shell. Point your editor to the
`rust-analyzer` binary on `$PATH` while the shell is active.

For direnv users:

```bash
echo "use flake" > .envrc
direnv allow
```

### Linting & formatting

```bash
cargo clippy --workspace
cargo fmt
cargo fmt -- --check    # check formatting only (CI)
```

### Update dependencies

```bash
nix develop
cargo update            # bump Cargo.lock to latest semver-compatible
nix flake update        # bump nixpkgs revision
```

After `cargo update` you must rebuild with `nix build` to refresh the Nix
vendor hash. The `cargoLock.lockFile` approach used here means no manual hash
management — Nix reads `Cargo.lock` directly.

## How the pieces fit together

### GTK (GUI)

The `lorebird-gtk` crate provides the GTK4 UI. It depends on `lorebird-core`
for all email logic and `lorebird-lua` for scripting/config.

### Lua (configuration & hooks)

`lorebird-lua` wraps `mlua` (Lua 5.4, vendored). Configuration, fetch hooks,
reply templates, and send hooks are all expressed in Lua — much like neovim's
approach. The VM is started at app launch and exposes APIs for maildir
operations, searching, and UI callbacks.

### SQLite + FTS5

`lorebird-core` uses `rusqlite` (v0.39, bundled) with FTS5 for full-text
search. The schema module creates tables, triggers, and the FTS5 virtual table.
The query module provides a Xapian-style query parser that compiles to FTS5
MATCH expressions.

### Mail parsing

`lorebird-core` uses `mail-parser` (Stalwart Labs, v0.10) to parse raw email
bytes. The `message.rs` module extracts the fields we care about (Message-ID,
References, Subject, From, Date, body) and implements the `thread::Message`
trait so parsed messages can be fed directly to the JWZ threading algorithm.

### Maildir indexing

`lorebird-core` walks the maildir filesystem directly (no `maildir` crate) —
iterating `cur/` and `new/` subdirectories, reading each file as raw bytes,
and passing them to `mail_parser`. The `maildir-index` CLI tool exercises this
code path independently of the GUI.

## Building without Nix

If you don't use Nix, install system packages manually:

```bash
# Ubuntu / Debian
sudo apt install libgtk-4-dev libsqlite3-dev pkg-config

# Fedora
sudo dnf install gtk4-devel sqlite-devel pkg-config

# macOS
brew install gtk4 sqlite3 pkg-config
```

Then use standard Cargo commands:

```bash
cargo build --release
./target/release/lorebird
```
