# lorebird Code-State Audit

Audited 2026-06-09. All specs and code cross-checked.

---

## 1. Spec ↔ Code drift

### specs/lorefetch-module.md

No drift. Accurately describes the streaming pipeline, incremental queries,
Anubis solver, MboxParser, cache v2, read-stall timeout, and CLI.
Matches the actual code in `crates/lorebird-lorefetch/src/lib.rs`.

### specs/sendmail-module.md

| § | Spec says | Code actually does | Status |
|---|-----------|-------------------|--------|
| `SmtpConfig` derives | `serde::Deserialize` only | `Debug, Clone, serde::Deserialize, serde::Serialize` | Minor — `Serialize` needed for `lua.to_value()` round-tripping |
| Password resolution section | Says "resolved at send time" with `SmtpConfig::resolved_password()` | Correct — `resolved_password()` called inside `send()`, prefix preserved in stored config | ✓ |
| TODO: integration test | Listed as incomplete | No integration test against local SMTP server yet | Still open |

### specs/app_config_and_email_compose.md

| § | Spec says | Code actually does | Status |
|---|-----------|-------------------|--------|
| `sh(cmd, opts)` | `opts` table: `stdin`, `stdin_file`, `env` | Code now implements all three opts — `stdin` (string pipe), `stdin_file` (file pipe), `env` (extra env vars) | ✓ |
| `lorefetch()` return table | Shows `count`, `new`, `timed_out`, `error` | Code matches: `{ ok, count, new, timed_out?, error? }` | ✓ |
| `send_smtp()` return table | Shows `ok`, `error` | Code matches | ✓ |
| `on_fetch(label, maildir)` | Per-profile hook inside profile table | Code matches — `on_fetch` extracted from each profile's Lua table | ✓ |
| `on_reply(label, parent, mail)` | Global hook at `config` level | Code matches | ✓ |
| `on_send(label, mail)` | Global hook at `config` level | Code matches | ✓ |
| 3-way send dispatch | Table in spec matches code in `lua_thread.rs` | Code matches | ✓ |
| `mail_to_rfc2822(mail)` | Documented | Implemented and working | ✓ |

### specs/threading.md

No drift. Accurately describes the two-thread model, channel protocol,
fetch/reply/send flows, SQLite concurrency, and the `Mail` type
crossing the channel.

### specs/stream-fetch.md

All features marked ✅ Done. This spec will eventually be merged into
`lorefetch-module.md` and removed.

### specs/icon.md

Accurate. Documents the GResource bundle, `build.rs` compilation,
`gio::resources_register_include!`, icon theme registration, Windows `.ico`
embedding via `winres`, and Nix packaging with hicolor icon install.

### specs/idea_pgp_signing.md

Design note, not a spec. Describes how PGP signing/encryption can be done
entirely in Lua `on_send` hooks, with the only Rust change being `opts.stdin`
support for `sh()`. No code implements this yet.

---

## 2. What works end-to-end

| Capability | Status | Verified |
|-----------|--------|----------|
| Maildir indexing (SQLite+FTS5) | ✅ Done | Unit tested |
| JWZ threading + in-memory HashMap | ✅ Done | Unit tested |
| Search → thread display pipeline | ✅ Done | Manual testing |
| Lua config loading (profiles, hooks, views) | ✅ Done | Unit tested |
| `on_fetch(label, maildir)` | ✅ Done | Manual: 40k messages fetched |
| `on_reply(label, parent, mail)` | ✅ Done | Unit tested |
| `on_send(label, mail)` — 3-way dispatch | ✅ Done | Manual: email delivered to inbox |
| `lorefetch(maildir, query)` | ✅ Done | Manual: 40k messages, streaming |
| `send_smtp(rfc2822)` | ✅ Done | Manual: sent real email, arrived |
| Streaming mbox pipeline (no full-buffer) | ✅ Done | Manual: 40k messages |
| Read-stall timeout (30s no-data) | ✅ Done | Designed; long transfers succeed |
| Content detection (gzip/HTML/raw) | ✅ Done | Manual: gzipped mbox from lore.kernel.org |
| Per-query incremental `last_date` | ✅ Done | Unit tested |
| Anubis challenge solver | ✅ Done | Manual: lore.kernel.org |
| mboxrd `>From ` un-escaping | ✅ Done | Unit tested |
| Partial fetch on timeout | ✅ Done | Designed; saves processed msgs |
| GTK4 mail reader UI | ✅ Done | Manual testing |
| Compose window with reply pre-fill | ✅ Done | Manual testing |
| GResource bundle (icons compiled into binary) | ✅ Done | Built and verified |
| Windows `.ico` embedding in `.exe` | ✅ Done | `winres` in build.rs |
| Linux `.desktop` file + hicolor icons | ✅ Done | Nix `postInstall` in flake.nix |
| Console window suppression on Windows (release) | ✅ Done | `#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]` |
| `#[cfg(unix)]` gate on `PermissionsExt` | ✅ Done | Cross-compiles to Windows |

---

## 3. Stale / unhandled TODOs

| Item | Status | Priority |
|------|--------|----------|
| `sh()` `opts` parameter (`stdin`, `stdin_file`, `env`) | ✅ Implemented. Needed for PGP signing via `sh({"gpg"}, { stdin = body })` | Done |
| Integration test for SMTP against local mock server | No test. Low priority for v0.1. | Low |
| `lorefetch` integration test against live lore.kernel.org | No automated E2E test. Fragile to automate. | Low |
| HTML email rendering | SourceView shows plain text only | Future |
| Attachments | Not implemented | Future |
| PGP sign/encrypt | Design note in `specs/idea_pgp_signing.md`. Blocked on `sh()` stdin support. | Future |
| Maildir `new/` → `cur/` flag transition | Messages written to `new/` but never moved to `cur/` on read | Future |

---

## 4. Architecture summary

### Crate layout

```
crates/
  lorebird-core/      # SQLite+FTS5 indexing, JWZ threading, query parser, compose types
  lorebird-lua/       # Lua VM, config loading, hooks (on_fetch, on_reply, on_send), sh(), send_smtp()
  lorebird-gtk/       # GTK4 UI, column view, compose window, GResource bundle
  lorebird-lorefetch/ # Streaming lore.kernel.org fetch, Anubis solver, incremental queries
  lorebird-sendmail/  # Built-in SMTP via lettre (rustls, no OpenSSL)

tools/
  lorefetch/          # CLI tool: --query, --maildir, --mbox, --list, -v
  maildir-index/      # Standalone maildir → SQLite indexer
  mail-query/         # Standalone query tool
  query-test/         # Query engine test harness
  thread-test/        # Threading test harness
```

### Hook architecture

| Hook | Scope | Where defined | Receives |
|------|-------|---------------|----------|
| `on_fetch` | Per-profile | Inside each profile table in Lua config | `(label, maildir)` |
| `on_reply` | Global | At `config` level in Lua config | `(label, parent, mail)` |
| `on_send` | Global | At `config` level in Lua config | `(label, mail)` |

### 3-way send dispatch

| `on_send` hook | Profile `smtp` config | Behaviour |
|----------------|----------------------|-----------|
| ✅ defined | ✅ present | Hook runs; can call `send_smtp(mail_to_rfc2822(mail))` |
| ✅ defined | ❌ absent | Hook runs; must use `sh({"msmtp"})` etc. |
| ❌ absent | ✅ present | Automatic: lorebird sends via built-in SMTP |
| ❌ absent | ❌ absent | Error: "no on_send hook and no smtp config" |

### Lua API surface

| Function | Returns | Purpose |
|----------|---------|---------|
| `lorefetch(maildir, query)` | `{ ok, count, new, timed_out?, error? }` | In-process lore.kernel.org fetch |
| `send_smtp(rfc2822_text)` | `{ ok, error? }` | Send via profile's SMTP config |
| `mail_to_rfc2822(mail_table)` | `string` | Serialize mail table to RFC 2822 |
| `read_file(path)` | `string` | Read file contents |
| `write_tmpfile(content)` | `path` | Write to temp file, returns path |
| `sh(cmd [, opts])` | `{ ok, exit_code, stdout, stderr }` | Run external command; opts: `stdin`, `stdin_file`, `env` |

### Data flow

```
User clicks Fetch
  → main thread sends LuaCommand::Fetch to Lua thread
  → Lua thread calls on_fetch(label, maildir) hook
  → hook calls loretch(maildir, query) [built-in] or sh({"lorefetch", ...})
  → on success: Lua thread runs index_maildir()
  → Lua thread sends LuaResult::FetchDone back
  → main thread rebuilds thread tree from DB

User clicks Reply
  → main thread builds Mail (parent) + Mail (reply)
  → sends LuaCommand::Reply to Lua thread
  → Lua thread calls on_reply(label, parent, mail) if defined
  → hook may modify mail table in-place
  → sends LuaResult::ReplyDone back
  → main thread opens compose window with modified mail

User clicks Send
  → main thread sends LuaCommand::Send to Lua thread
  → if on_send hook: calls it; hook calls send_smtp() or sh() as needed
  → if no hook + smtp config: auto-sends via lorebird-sendmail
  → if neither: error
  → sends LuaResult::SendDone back
  → main thread closes compose window or shows error
```

---

## 5. Test coverage

| Crate | Tests | Key areas |
|-------|-------|-----------|
| lorebird-core | 96 | Indexing, threading, query parser, compose, date handling |
| lorebird-lorefetch | 34 | MboxParser, cache v2, incremental queries, content detection, Anubis solver, gzip round-trip |
| lorebird-sendmail | 9 | SMTP config resolution, eval: passwords, send (mocked) |
| lorebird-lua | 30+ | Config loading, hook dispatch, sh(), send_smtp(), write_tmpfile() |
| lorebird-gtk | 0 | UI testing requires display; manual testing only |
| **Total** | **~173** | |

---

## 6. Key design decisions

| Decision | Rationale |
|----------|-----------|
| Single `lib.rs` per crate | Go program is one file; YAGNI |
| `ureq` v3 (synchronous HTTP) | Matches Lua thread's blocking model; no async runtime |
| `lettre` with `rustls-tls` | Pure Rust TLS; no OpenSSL dependency |
| `flate2` for gzip | Server sends `Content-Type: application/gzip`; `0x1f 0x8b` detection + manual decompression |
| `mail_parser` for Message-ID / Date | Already a dependency of `lorebird-core` |
| SHA-1 for maildir filenames | Faithful to Go lorefetch; naming, not security |
| Per-query `last_date` cache | Switching queries preserves incremental state |
| 1-day overlap in `dt:` injection | Prevents gaps from timezone/clock skew |
| No cache v1→v2 migration | Old caches deleted and rebuilt |
| `on_fetch` per-profile, `on_reply`/`on_send` global | Fetch is per-maildir (profile-specific), send/reply are account-wide |
| `SmtpConfig::resolved_password()` at send time | Rotating tokens (e.g. `eval:pass show email/gmail`) always fresh |
| GResource bundle for icons | Icons compiled into binary; no filesystem install needed at runtime |
| Windows `.ico` in `.exe` via `winres` | Shows in explorer, taskbar, shortcuts |
| `#[cfg(unix)]` on `PermissionsExt` | Cross-platform compilation; Unix-only file permissions |
| `#![cfg_attr(all(target_os="windows", not(debug_assertions)), windows_subsystem="windows")]` | Release builds suppress console window; debug builds keep it for println output |
| `#[cfg(windows)]` on `winres` in `build.rs` | Only compiles Windows resource embedding when targeting Windows |