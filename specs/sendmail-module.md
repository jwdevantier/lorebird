# Sendmail Module — Design Specification

## Purpose

Provide built-in SMTP sending via the `lettre` crate, eliminating the need
for an external MTA like `msmtp`.  This makes lorebird a turn-key solution:
fetch (lore.kernel.org via `lorefetch`) and send (SMTP via `send_smtp`)
without external tools beyond the binary itself.

The Lua `on_send` hook is retained.  Users who prefer `msmtp` or
`sendmail` can keep using `sh({...})` in their hook.  `send_smtp()` is a
new API helper they can call *inside* `on_send` — or, if no `on_send`
hook is defined but the profile has an `smtp` block, lorebird sends
automatically.

---

## Workspace layout

```
crates/lorebird-sendmail/
  Cargo.toml
  src/
    lib.rs          # SmtpConfig, send(), public API
```

One source file, mirroring the `lorefetch` crate organisation.

---

## Dependencies

```toml
# crates/lorebird-sendmail/Cargo.toml
[package]
name = "lorebird-sendmail"
version = "0.1.0"
edition = "2024"

[dependencies]
lettre = { version = "0.11", default-features = false, features = ["builder", "smtp-transport", "rustls-tls", "hostname"] }
serde = { version = "1", features = ["derive"] }
thiserror = "2"

[dev-dependencies]
tempfile = "3"
serde_json = "1"
```

> No `tokio` — lettre's blocking `Transport` trait is synchronous.
> `rustls-tls` gives us pure-Rust TLS; no OpenSSL or native-tls needed.
> `hostname` lets lettre default the EHLO name sensibly.

---

## Error type

```rust
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("SMTP error: {0}")]
    Smtp(String),

    #[error("Invalid address: {0}")]
    Address(String),

    #[error("Missing SMTP configuration")]
    NoConfig,

    #[error("Password evaluation failed: {0}")]
    CommandEval(String),
}
```

---

## Configuration

Each profile has its own `smtp` block.  No global fallback, no merging,
no precedence questions — identical to how `maildir` already works.

If multiple profiles share the same SMTP server, define it as a Lua
variable and reference it:

```lua
local gmail = {
  host     = "smtp.gmail.com",
  port     = 587,
  username = "me@gmail.com",
  password = "eval:pass show email/gmail",
  starttls = true,
}

config = {
  user = { name = "Riccardo Maffulli", email = "riccardo@defmacro.it" },

  profiles = {
    ["qemu"] = {
      maildir = "/home/nixos/loremail/qemu",
      smtp    = gmail,
      on_fetch = function(label, maildir)
        return lorefetch(maildir, "l:qemu-devel AND f:me@gmail.com").ok
      end,
    },
    ["linux"] = {
      maildir = "/home/nixos/loremail/kernel",
      smtp    = gmail,
      on_fetch = function(label, maildir)
        return lorefetch(maildir, "l:linux-kernel AND f:me@gmail.com").ok
      end,
    },
    ["work"] = {
      maildir = "/home/nixos/loremail/work",
      smtp = {                       -- different server, defined inline
        host     = "smtp.work.com",
        port     = 465,
        username = "riccardo@work.com",
        password = "eval:pass show email/work",
        starttls = false,            -- SMTPS (port 465)
      },
      on_fetch = function(label, maildir)
        return lorefetch(maildir, "l:internals AND f:riccardo@work.com").ok
      end,
    },
  },
}
```

### Why no global `smtp` fallback

Lorebird's config is Lua — a programming language.  Users have
variables, functions, and composition.  A global-with-override pattern
(like msmtp's `account default`) adds merging rules and precedence
questions that aren't needed when the user can simply write
`smtp = my_shared_config`.  Keeping the model flat — each profile owns
its `smtp` outright — eliminates an entire class of "what takes
precedence?" confusion.

---

## `SmtpConfig` struct (Rust)

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_starttls")]
    pub starttls: bool,
}

fn default_port() -> u16 { 587 }
fn default_starttls() -> bool { true }
```

### Password evaluation

The `password` field supports two forms, resolved at **send time**
by `SmtpConfig::resolved_password()`:

| Value                              | Behaviour                              |
|------------------------------------|----------------------------------------|
| `"s3cret"`                         | Literal string                         |
| `"eval:pass show email/gmail"`     | Shell command; stdout is the password   |

The `eval:` / `sh:` prefix is stripped, the remainder is executed via
`sh -c`, and trailing newlines are trimmed from stdout.  Literal strings
(without prefix) pass through unchanged.

This is done in Rust during `load_config_string()` / `load_config_file()`,
not in Lua.  The resolved password is stored in `SmtpConfig.password` as
a plain string and never written back to disk.

> **Security note:** passwords with `eval:` / `sh:` prefixes are
evaluated on each send call.  This means rotating tokens (e.g. from
`pass show email/gmail`) are always fresh.  The resolved value is held
in memory only for the duration of the SMTP transaction.

---

## Public API

```rust
/// Send an RFC 2822 message via SMTP.
///
/// `rfc2822` is the complete message text (headers + blank line + body).
/// `envelope_from` and `envelope_to` are used for the SMTP envelope
/// (they may differ from the headers).
pub fn send(
    config: &SmtpConfig,
    envelope_from: &str,
    envelope_to: &[&str],
    rfc2822: &[u8],
) -> Result<(), SendError>;
```

The blocking `Transport::send_raw()` call is safe to use from the Lua
thread (which is already a dedicated blocking thread).

---

## Lua API surface

### `send_smtp(rfc2822_text) → { ok, error? }`

Sends an RFC 2822 formatted message via the profile's SMTP server.

```lua
on_send = function(profile, mail)
    local result = send_smtp(mail_to_rfc2822(mail))
    if not result.ok then
        error("Send failed: " .. result.error)
    end
end
```

Returns a table:

| Key     | Type    | Present | Meaning                        |
|---------|---------|---------|--------------------------------|
| `ok`    | boolean | always  | `true` on success              |
| `error` | string  | failure | Human-readable error message   |

**Error handling:** if no `smtp` config is available for the current
profile, `send_smtp()` returns `{ ok = false, error = "Missing SMTP configuration" }`.
It does **not** raise a Lua error — the caller decides how to handle
failure.

---

## Automatic sending (no `on_send` hook)

If a profile has **no `on_send` hook** but **does have an `smtp` block**,
lorebird sends automatically:

1. Build `ComposeMail` from the compose window
2. Call `mail_to_rfc2822(mail)`
3. Call `lorebird_sendmail::send(smtp_config, from, to, rfc2822)`

This makes the common case (SMTP send to a single server) zero-config
in Lua — just define `smtp` and `maildir`, skip `on_send` entirely.

If there is an `on_send` hook, it takes full control (as before).

---

## Interaction with `on_send`

| `on_send` hook | `smtp` block | Behaviour                                              |
|---------------|-------------|--------------------------------------------------------|
| ✅ defined     | ✅ present  | Hook runs; can call `send_smtp()` or `sh({"msmtp"})`  |
| ✅ defined     | ❌ absent   | Hook runs; must use `sh({"msmtp"})` etc.               |
| ❌ absent      | ✅ present  | Automatic: lorebird sends via built-in SMTP             |
| ❌ absent      | ❌ absent   | Error: "no on_send hook and no smtp config"             |

Case 3 is the turn-key path: define `smtp`, skip `on_send`, sending
just works.

---

## Resolved config (Rust side)

`ResolvedProfile` gains an optional `smtp` field:

```rust
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub label: String,
    pub name: String,
    pub email: String,
    pub maildir: PathBuf,
    pub views: Vec<ViewConfig>,
    pub smtp: Option<SmtpConfig>,  // NEW
}
```

Resolution: `profile.smtp` comes directly from the profile's Lua table.
No fallback, no merging.

---

## Data flow (send path)

```
┌──────────────────────────────────────────────────────┐
│  User clicks "Send" in compose window                │
│  → ComposeMail struct built from form fields          │
└────────────────────┬─────────────────────────────────┘
                     │
                     ▼
┌──────────────────────────────────────────────────────┐
│  LuaThread: SendCommand                               │
│                                                      │
│  if on_send hook exists:                             │
│    call on_send(profile, mail_table)                 │
│      → user may call send_smtp(rfc2822) in Lua       │
│      → or sh({"sendmail", "-t"})                     │
│                                                      │
│  else if profile.smtp is Some:                       │
│    mail_to_rfc2822(mail)                             │
│    lorebird_sendmail::send(smtp_config, from, to, rfc)│
│                                                      │
│  else:                                               │
│    error: "no on_send hook and no smtp config"        │
└──────────────────────────────────────────────────────┘
```

---

## Differences from msmtp

| Feature                 | msmtp                  | lorebird-sendmail         |
|-------------------------|------------------------|---------------------------|
| Config format           | `~/.msmtprc`           | `config.lua` `smtp` block |
| Multiple accounts      | `account` sections     | Per-profile `smtp` blocks  |
| Shared config reuse    | `account default`      | Lua variables              |
| Password eval           | `passwordeval`         | `eval:` prefix             |
| TLS                     | OpenSSL                | rustls (pure Rust)        |
| SMTPS / STARTTLS        | ✅                      | ✅                          |
| Auth mechanisms         | PLAIN, LOGIN, XOAUTH2  | PLAIN, LOGIN (lettre)     |
| Logging / syslog        | ✅                      | eprintln (currently)      |
| Queue / retry           | ✅ (msmtpq)            | ❌ (single attempt)         |

The only gap vs. msmtp is XOAUTH2 auth and queue/retry.  These are
unlikely to be needed for mailing-list workflows (the primary use case).
PLAIN and LOGIN cover all standard SMTP servers.

---

## TODO

- [x] Create `crates/lorebird-sendmail/` with `Cargo.toml` and `lib.rs`
- [x] Implement `SmtpConfig` struct and `send()` function
- [x] Add `smtp` field to `ProfileData` (per-profile, no global)
- [x] Password evaluation (`eval:` / `sh:` prefix) at send time
- [x] Register `send_smtp()` Lua function in `lorebird-lua`
- [x] Add `SmtpConfig` to `ResolvedProfile` + resolve logic
- [x] Update `lua_thread.rs` send path: auto-send if no `on_send` hook
- [x] Update `specs/app_config_and_email_compose.md` with `smtp` config block
- [x] Add `lorebird-sendmail` to workspace `Cargo.toml`
- [ ] Integration test against a local SMTP server (e.g., mailhog)