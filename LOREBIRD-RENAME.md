# lorebird → lorebird Rename Checklist

## 1. Directory names (crate dirs must move)

| Current | New |
|---|---|
| `crates/lorebird-core/` | `crates/lorebird-core/` |
| `crates/lorebird-lua/` | `crates/lorebird-lua/` |
| `crates/lorebird-gtk/` | `crates/lorebird-gtk/` |
| `crates/lorebird-lorefetch/` | `crates/lorebird-lorefetch/` |
| `crates/lorebird-sendmail/` | `crates/lorebird-sendmail/` |

Use `git mv` to preserve history.

## 2. Cargo.toml — package names + dependency paths

**Workspace root** (`Cargo.toml`): update all 5 `members` paths.

**Each crate's `Cargo.toml`**: rename `name = "lorebird-*"` → `"lorebird-*"`, and update every `path = "../lorebird-*"` dependency. ~15 cross-crate dependency references across:

- `crates/lorebird-gtk/Cargo.toml` → depends on `lorebird-core`, `lorebird-lua`
- `crates/lorebird-lua/Cargo.toml` → depends on `lorebird-core`, `lorebird-lorefetch`, `lorebird-sendmail`
- All 5 tools under `tools/*/Cargo.toml` → depend on `lorebird-core` or `lorebird-lorefetch`

**`Cargo.lock`**: delete and regenerate (`cargo check`). Do not hand-edit.

## 3. Rust `use` and `extern crate` statements

Every `use lorebird_core::`, `use lorebird_lua::`, `lorebird_lorefetch::`, `lorebird_sendmail::`, and `lorebird_core` function call must become `lorebird_*`. Affected files (~12 `.rs` files):

- `crates/lorebird-gtk/src/{app_state,lua_thread,compose,main,window}.rs`
- `crates/lorebird-lua/src/{lib,config}.rs`
- `tools/{query-test,lorefetch,maildir-index,thread-test,mail-query}/src/main.rs`

## 4. Rust identifiers / function names

These are **public API surface** that includes "lorebird" in the name itself:

| File | Current | Must rename to |
|---|---|---|
| `config_dir.rs` | `lorebird_confdir()` | `lorebird_confdir()` |
| `config_dir.rs` | `lorebird_conf_path()` | `lorebird_conf_path()` |
| `lua_thread.rs` | `dirs_for_lorebird()` | `dirs_for_lorebird()` |
| `lua_thread.rs` | `lorebird_conf_path` param | `lorebird_conf_path` |

Also update all callers of these functions (in `window.rs`, `lua_thread.rs`, tests in `config_dir.rs`).

## 5. String literals (user-visible & config paths)

These are **not** just find-and-replace — they affect **on-disk state**:

| Where | Current | Note |
|---|---|---|
| `config_dir.rs` | `"lorebird"` in path joins | ⚠️ Changes config dir from `~/.config/lorebird/` → `~/.config/lorebird/` |
| `app_state.rs` | `".lorebird.db"` | ⚠️ Changes DB filename in each maildir |
| `compose.rs` | `"X-Mailer: lorebird"` | Email header — user-visible to recipients |
| `lib.rs` | `"_lorebird_smtp"` | Lua global name — **breaks existing user Lua configs** |
| `lib.rs` | `format!("lorebird_{}_{}", pid, count)` | Temp file prefix |
| `window.rs` | `"<config dir>/lorebird/config.lua"` | Error message |

**Critical**: If users already have `~/.config/lorebird/config.lua` and `.lorebird.db` files, renaming the paths will silently create a fresh config/DB. Consider a migration path or keeping the old paths as fallback.

## 6. GLib/GTK application identity

| File | Current | Must become |
|---|---|---|
| `main.rs` | `gio::resources_register_include!("org.lorebird.app.gresource")` | `"org.lorebird.app.gresource"` |
| `main.rs` | `.application_id("org.lorebird.app")` | `"org.lorebird.app"` |
| `main.rs` | `.add_resource_path("/org/lorebird/app/icons")` | `"/org/lorebird/app/icons"` |
| `build.rs` | `"resources/org.lorebird.app.gresource.xml"` | `"resources/org.lorebird.app.gresource.xml"` |
| `build.rs` | `"org.lorebird.app.gresource"` | `"org.lorebird.app.gresource"` |
| `build.rs` | `"resources/org.lorebird.app.ico"` | `"resources/org.lorebird.app.ico"` |
| `window.rs` | `.icon_name("org.lorebird.app")` | `"org.lorebird.app"` |

## 7. Icon files & gresource XML

Rename all `icon/org.lorebird.app.*.png` → `org.lorebird.app.*.png` and same in `crates/lorebird-gtk/resources/`. Update `org.lorebird.app.gresource.xml` → `org.lorebird.app.gresource.xml` with new filenames and **new prefix** `/org/lorebird/app`.

## 8. Desktop file

`dist/org.lorebird.app.desktop` → `dist/org.lorebird.app.desktop`: change `Name`, `Exec`, `Icon` fields.

## 9. flake.nix

- `description` string
- Shell `name = "lorebird-dev"`
- Shell hook `echo "=== lorebird dev shell ==="`
- `pname = "lorebird"`
- Desktop item `name = "org.lorebird.app"`, `exec`, `icon`, `desktopName`
- `postInstall` icon paths (`org.lorebird.app.*.png`)
- Package attribute names `default = lorebird; lorebird = lorebird;`

## 10. GitHub Actions

`.github/workflows/build.yml`:
- `cargo build --release -p lorebird` → `-p lorebird`
- `bash scripts/win-ucr64t-bundler.sh target/release/lorebird.exe dist/lorebird`
- Icon copy: `cp crates/lorebird-gtk/resources/org.lorebird.app.ico`
- Artifact name: `lorebird-windows-x86_64`
- Release zip: `lorebird-${{ github.ref_name }}-windows-x86_64.zip`

## 11. Documentation & specs

`README.md`, `DEV.md`, `docs/DEV.md`, `code-state.md`, `IMPLEMENTATION_TODO.md`, and all `specs/*.md` files contain "lorebird" in prose.

## 12. The `lorefetch` crate/tool naming

`lorefetch` is a portmanteau of "lore" + "fetch", **not** "lorebird" + "fetch". The crate is named `lorebird-lorefetch` and the tool is `tools/lorefetch/`. Decision needed:

- Crate: `lorebird-lorefetch` → `lorebird-lorefetch` (hyphenated brand)
- Tool dir: `tools/lorefetch/` — keep as-is since it's conceptually "lore + fetch"

## Recommended order

1. **`git mv` all crate/tool directories** first
2. **Bulk replace `lorebird` → `lorebird`** in all source files (Cargo.toml, .rs, .nix, .yml, .md, .desktop, .xml)
3. **Handle the `lorefetch` ambiguity** — decide if the tool stays `lorefetch` or becomes `lorebird-lorefetch`
4. **Rename icon/resource/desktop files** on disk
5. **Delete and regenerate `Cargo.lock`**
6. **`cargo check --workspace`** to verify
7. **`cargo test --workspace`** to confirm
8. **`nix build`** to verify the flake
9. **Commit with a clear message** like `rename: lorebird → lorebird`

## Biggest footgun

The on-disk config dir (`~/.config/lorebird/`) and `.lorebird.db` path changes will **orphan existing user data**. A migration path or fallback should be considered.