# loreread

**Index and browse a maildir with GTK, Guile, and SQLite.**

A desktop application built with Rust and GTK4, embedding a Guile Scheme runtime
for extensibility, backed by SQLite with full-text search (FTS5), and capable of
parsing maildir archives.

## Quick start

```bash
# Build the release binary
nix build

# Run it
./result/bin/loreread

# Or enter the dev shell and cargo-run
nix develop
cargo run
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
| `pkg-config` | Locate system libraries (GTK, Guile) |
| `gtk4` (dev) | GTK4 headers & libraries for gtk-rs |
| `guile` (dev) | GNU Guile runtime & C headers for FFI |
| `sqlite3` | SQLite CLI (`sqlite-interactive`) |

All library search paths are set up automatically — `cargo build`, `cargo run`,
and `cargo test` work out of the box.

### Edit-Compile-Run loop

```bash
nix develop          # enter shell once

cargo check          # fast compile check, no binary
cargo build          # debug build
cargo run            # build + run in one step
cargo test           # run unit tests

# Release build (same as `nix build`)
cargo build --release
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
cargo clippy         # lint with helpful suggestions
cargo fmt            # format all Rust sources
cargo fmt -- --check # check formatting only (CI)
```

### Update dependencies

```bash
nix develop
cargo update         # bump Cargo.lock to latest semver-compatible
nix flake update     # bump nixpkgs revision
```

After `cargo update` you must rebuild with `nix build` to refresh the Nix
vendor hash. The `cargoLock.lockFile` approach used here means no manual hash
management — Nix reads `Cargo.lock` directly.

## Project structure

```
loreread/
├── flake.nix          # Nix dev shell + package definition
├── Cargo.toml         # Rust crate manifest
├── Cargo.lock         # Pinned dependency versions
└── src/
    └── main.rs        # GTK4 UI + Guile FFI entry point
```

## How the pieces fit together

### GTK (GUI)

The `gtk4` Rust crate (v0.9, gtk-rs) provides safe Rust bindings to GTK4.
A window is created with a vertical box containing a button and a label.
Clicking the button triggers a Guile Scheme evaluation and displays the result.

### Guile (embedded Scheme)

Guile is linked via raw FFI (`#[link(name = "guile-3.0")]`). On startup,
`scm_init_guile()` boots the interpreter. On button click, a Scheme expression
is evaluated with `scm_c_eval_string()`, and the result string is extracted via
`scm_to_locale_string()`.

The `guile` module inside `src/main.rs` wraps these C calls in a safe
`guile::eval(&str) -> Result<String, String>` function.

### SQLite + FTS5

`rusqlite` (v0.39) with the `bundled` feature compiles its own copy of SQLite
from source, enabling FTS5, JSON1, and other extensions. The crate is available
as a dependency and ready to use — just add `use rusqlite::Connection;` and
open a database.

### Maildir parsing

The `maildir` crate (v0.6) provides a pure-Rust maildir reader. Use it to
iterate over messages in a maildir tree:

```rust
use maildir::Maildir;

let md = Maildir::from("/path/to/maildir");
for entry in md.list_new() {
    let mail = entry.unwrap().parsed().unwrap();
    println!("From: {:?}", mail.headers.get_first("From"));
}
```

## Building without Nix

If you don't use Nix, install system packages manually:

```bash
# Ubuntu / Debian
sudo apt install libgtk-4-dev guile-3.0-dev libsqlite3-dev pkg-config

# Fedora
sudo dnf install gtk4-devel guile-devel sqlite-devel pkg-config

# macOS
brew install gtk4 guile sqlite3 pkg-config
```

Then use standard Cargo commands:

```bash
cargo build --release
./target/release/loreread
```