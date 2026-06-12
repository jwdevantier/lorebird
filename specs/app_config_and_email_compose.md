# Application Configuration & Email Compose

## Overview

Lorebird is configured through a Lua script. The configuration defines
**profiles** (maildir accounts), **views** (named saved queries), and
**hooks** (Lua functions called at key moments). The UI is a Thunderbird-style
graphical mail reader; composition of replies is handled by a built-in editor
window pre-filled from the parent message.

---

## Configuration schema

### Global defaults

```lua
config = {
  user = {
    name = "Riccardo Maffulli",       -- fallback for profile.name
    email = "riccardo@defmacro.it",   -- fallback for profile.email
  },

  profiles = { ... },                 -- see below

  on_reply = function(profile, parent, mail) ... end,   -- optional
  on_send  = function(profile, mail) ... end,            -- see below
}
```

`user.name` and `user.email` serve as **global defaults**.  Any profile that
omits `name` or `email` inherits from these.  A profile that defines its own
`name` / `email` overrides the global value for that profile alone — exactly
like git's local-vs-global config resolution.

### Profiles

Each profile key is a **human-readable label** (the display name in the
sidebar).  This key is also the hashmap key — duplicate labels are impossible
by construction.

```lua
config = {
  profiles = {
    ["qemu nvme"] = {
      -- Identity (overrides global user.name / user.email)
      name  = "Riccardo Maffulli",     -- optional; falls back to global
      email = "riccardo@defmacro.it",  -- optional; falls back to global

      -- Maildir path (required)
      maildir = "/home/nixos/loremail/INBOX",

      -- SMTP config (optional; enables send_smtp() in on_send and
      -- automatic sending if no on_send hook is defined)
      smtp = {
        host     = "smtp.gmail.com",    -- SMTP server hostname
        port     = 587,                  -- 465 for SMTPS, 587 for STARTTLS
        username = "riccardo@gmail.com",
        password = "eval:pass show email/gmail",  -- or a literal string
        starttls = true,                 -- true = STARTTLS; false = SMTPS
      },

      -- Fetch hook (required)
      on_fetch = function(label, maildir)
        return sh({
          "lorefetch",
          "-query", "l:qemu-devel AND (c:foss@defmacro.it OR ...)",
          "-maildir", maildir,
        })
      end,

      -- Saved views (optional)
      views = {
        { label = "last week",  query = "date:1w.." },
        { label = "maintainer", query = "f:its@irrelevant.dk" },
      },
    },

    ["linux kernel"] = {
      -- Inherits name/email from global user.*
      maildir = "/home/nixos/loremail/kernel",
      on_fetch = function(label, maildir) ... end,
      views = {
        { label = " drm ",  query = "subject:drm" },
      },
    },
  },
}
```

### Views

A **view** is a named, stored query.  Clicking a view in the sidebar applies
its `query` string to filter the thread list.  Every profile also has an
implicit **"All Mail"** view with an empty query (no filter).

```lua
views = {
  { label = "last week",  query = "date:1w.." },
  { label = "maintainer", query = "f:its@irrelevant.dk" },
}
```

The query language is the one defined in `lorebird-core::query` — Xapian-style
with field prefixes and date ranges.

---

## Hooks

### `on_fetch(label, maildir) → truthy | falsy`

Called when the user clicks **Fetch** (or on startup, or via a timer).
Receives the profile label string and the maildir path string.

**Purpose:** Spawn an external process (lorefetch, lei, etc.) to bring new
mail into the maildir.

**Return value:**

| Return | Effect |
|--------|--------|
| truthy | Run the incremental indexer, then rebuild thread tree |
| falsy / nil | Show error in status bar; do not index |

The `sh()` API helper (see below) returns a table `{ ok, exit_code, stdout, stderr }`.
A simple hook can `return sh({...}).ok`.

**Indexing flow after successful fetch:**

1. Call `lorebird_core::indexer::index_maildir(conn, &maildir_path)`
2. This is incremental — `INSERT OR IGNORE` on the unique `filename` column
   skips already-indexed files
3. Rebuild the in-memory JWZ thread tree from the updated `mail_ndx` table
4. Refresh the `ColumnView`

Running the indexer when nothing changed is harmless (no-op), so there is no
need for file-count heuristics.

### `on_reply(profile, parent, mail)`

Called when the user clicks **Reply** on a message.  **Receives a pre-filled
mail object** — the Rust side has already done the standard reply mechanics:

| Field | How it's pre-filled |
|-------|---------------------|
| `from` | Set to `profile.name + " <" + profile.email + ">"` |
| `to` | `parent.from` (the person being replied to) |
| `cc` | `parent.to + parent.cc`, minus self, deduplicated |
| `subject` | `"Re: " + parent.subject` (with de-duplication of "Re:") |
| `date` | `nil` (auto-generated at send time) |
| `message_id` | Freshly generated `<YYYYMMDDHHMMSS.COUNTER-USER@DOMAIN>` |
| `in_reply_to` | Set to `parent.message_id` |
| `references` | Set to `parent.references + " " + parent.message_id` |
| `body_text` | Quoted body: `"\n\nOn <date>, <from> wrote:\n> <body>"` |
| `bcc` | Empty string by default |

**If the hook is absent**, the pre-filled mail is used as-is — no Lua code
required for a reasonable default.

**If the hook is present**, it receives `(profile, parent, mail)` and may
modify `mail` **in-place** — simply set fields on the table directly.  The
hook's return value is ignored; lorebird always extracts the modified mail
from the table argument after the hook returns.  This lets users customise
subject prefixes, add/remove CCs, change quoting style, or inject disclaimers
per mailing list.

### `on_send(profile, mail)`

Called when the user finishes composing a message and clicks **Send**.
`mail` is a **mail object** (the same kind of table as in `on_reply`) — **not**
a file path.  The hook can use `mail_to_rfc2822(mail)` to format the mail as
RFC 2822 text and `write_tmpfile(content)` to write it to a temporary file,
then pipe it to an external MTA via `sh()`.

**Purpose:** Deliver the mail to an external send mechanism.

```lua
on_send = function(profile, mail)
  -- Option A: format to RFC 2822, write to tmpfile, pipe to sendmail
  local rfc2822 = mail_to_rfc2822(mail)
  local fpath = write_tmpfile(rfc2822)
  sh({"sendmail", "-t"}, { stdin_file = fpath })

  -- Option B: built-in SMTP (no external binary needed)
  local result = send_smtp(mail_to_rfc2822(mail))
  if not result.ok then error("Send failed: " .. result.error) end
end
```

`on_send` is **optional** if the profile has an `smtp` config block — in that
case lorebird sends automatically.  If neither `on_send` nor `smtp` is
configured, the compose window will display an error message when Send
is clicked.

---

## Lua API surface

The following functions are exposed to the Lua VM by lorebird:

### `sh(cmd [, opts]) → result_table`

Synchronously execute an external command.

```lua
local result = sh({"lorefetch", "-query", "l:qemu-devel", "-maildir", "/path"})
-- result.ok        → boolean, true if exit code == 0
-- result.exit_code → number
-- result.stdout     → string
-- result.stderr     → string

-- Pipe stdin to the child process
local sig = sh({"gpg", "--armor", "--detach-sign"}, { stdin = mail.body_text })

-- Pipe a file to stdin
sh({"msmtp", "-t"}, { stdin_file = fpath })

-- Pass extra environment variables
sh({"pass", "show", "email/gmail"}, { env = { PASSWORD_STORE_DIR = "/custom/path" } })
```

`opts` (optional) may include:

| Key | Type | Meaning |
|-----|------|---------|
| `stdin` | string | String piped to child's stdin (takes priority over `stdin_file`) |
| `stdin_file` | string | Path to a file piped to child's stdin |
| `env` | table | Extra environment variables (merged with inherited env) |

The command runs **synchronously** (blocks the UI thread).  For long-running
fetches, the UI should show a progress indicator before calling the hook.
(See [Future: async fetch](#future-async-fetch) below.)

### `read_file(path) → string`

Read the entire contents of a file.  Convenience helper for use in `on_send`.

### `write_tmpfile(content) → path`

Write `content` to a unique temporary file and return the file path as a
string.  Temp files are created under the system temp directory
(e.g. `/tmp/lorebird_<pid>_<counter>`) and are cleaned up when the
application exits.

This is the primary way for `on_send` hooks to materialise an RFC 2822
message on disk before piping it to an external MTA like `sendmail`.

```lua
local fpath = write_tmpfile("Hello, world!")
-- fpath is something like "/tmp/lorebird_12345_0"
```

Note: the return value is a plain string (the file path), not a file handle.
The file persists on disk until the application exits or the OS reclaims the
temp directory.

### `lorefetch(maildir, query) → result_table`

Fetch mail from lore.kernel.org and write new messages into the given
maildir.  This is the in-process equivalent of running the `lorefetch`
CLI tool — no external binary needed.

```lua
on_fetch = function(label, maildir)
    local result = lorefetch(maildir, "l:qemu-devel AND (t:patch)")
    if result.ok then
        print(string.format("Fetched %d new message(s)", result.count))
    else
        print("Fetch failed: " .. result.error)
    end
    return result.ok
end
```

Returns a table:

| Key    | Type     | Present | Meaning                        |
|--------|----------|---------|-------------------------------  |
| `ok`   | boolean  | always  | `true` on success, `false` on error |
| `count`| number   | success | Number of new messages written  |
| `error`| string   | failure | Human-readable error message    |

The `list` parameter defaults to `/all/` (searches all lists).  If you
need to restrict to a specific list, use the CLI tool or call
`lorebird_lorefetch::LoreClient` directly from Rust.

### `send_smtp(rfc2822_text) → result_table`

Send an RFC 2822 formatted message via the current profile's SMTP
server.  The SMTP config is taken from the profile's `smtp` block
(set automatically before the `on_send` hook is called).

```lua
on_send = function(profile, mail)
    local result = send_smtp(mail_to_rfc2822(mail))
    if not result.ok then error("Send failed: " .. result.error) end
end
```

Returns a table:

| Key     | Type    | Present | Meaning                        |
|---------|---------|---------|--------------------------------|
| `ok`    | boolean | always  | `true` on success              |
| `error` | string  | failure | Human-readable error message   |

If no `smtp` config is available for the current profile, returns
`{ ok = false, error = "no smtp config for current profile" }`.

### `mail_to_rfc2822(mail) → string`

Convert a mail table to an RFC 2822 formatted string.  The `mail` argument
has the same structure as the `mail` table passed to `on_reply` / `on_send`
— it must contain at minimum `from`, `to`, `subject`, and `body_text` fields.

The output is a standards-compliant RFC 2822 message suitable for piping to
`sendmail -t` or any other MTA that reads from stdin.

```lua
-- Inside on_send:
local rfc2822 = mail_to_rfc2822(mail)
-- rfc2822 looks like:
-- Date: Thu, 29 May 2025 10:00:00 +0200
-- From: Alice <alice@example.com>
-- To: Bob <bob@example.com>
-- Subject: Re: Test
-- Message-ID: <20260529103000.0-alice@example.com>
-- In-Reply-To: <parent@example.com>
-- References: <grandparent@example.com> <parent@example.com>
-- MIME-Version: 1.0
-- Content-Type: text/plain; charset=utf-8
-- X-Mailer: lorebird
--
-- Hello
```

Header order: `Date`, `From`, `To`, `Cc` (if non-empty), `Bcc` (if non-empty),
`Subject`, `Message-ID`, `In-Reply-To` (if present), `References` (if present),
then `MIME-Version`, `Content-Type`, `X-Mailer` (auto-generated unless
overridden in `mail.headers`), then any arbitrary headers from `mail.headers`
in sorted order, then a blank line, then `body_text`.

### `parent` table (passed to `on_reply`)

Read-only table describing the message the user is replying to.
Same shape as the `mail` table (see [mail](#mail) type), but not modifiable:

```lua
parent = {
  from         = "Alice <alice@example.com>",
  to           = "list@example.com",
  cc           = "bob@example.com, carol@example.com",
  bcc          = "",
  subject      = "[PATCH v2] Fix memory leak",
  date         = "2024-01-15T10:30:00+00:00",
  message_id   = "<abc@def>",
  references   = "<parent@def> <other@def>",
  in_reply_to  = "<parent@def>",
  body_text    = "This patch fixes...\n",
  headers      = {         -- ALL headers from the original message on disk
    ["MIME-Version"] = "1.0",
    ["Content-Type"] = "multipart/signed; protocol=\"application/pgp-signature\"",
    ["X-Mailer"]      = "Evolution 3.48",
    ["List-Id"]       = "qemu development <qemu-devel.nongnu.org>",
    ["List-Post"]      = "<mailto:qemu-devel@nongnu.org>",
    -- ... every header from the original message, unfiltered
  },
}
```

### `mail` table (passed to `on_reply` and `on_send`, modifiable)

The pre-filled reply.  Contains all the fields from `parent`, plus
composition-specific fields:

```lua
mail = {
  from         = "Riccardo Maffulli <riccardo@defmacro.it>",
  to           = "Alice <alice@example.com>",
  cc           = "list@example.com, bob@example.com, carol@example.com",
  bcc          = "",   -- empty by default
  subject      = "Re: [PATCH v2] Fix memory leak",
  date         = nil,  -- auto-generated at send time
  message_id   = "<20240115103000.0-riccardo@defmacro.it>",
  in_reply_to  = "<abc@def>",
  references   = "<parent@def> <other@def> <abc@def>",
  body_text    = "\n\nOn 2024-01-15, Alice wrote:\n> This patch fixes...\n",
  headers      = {},   -- arbitrary extra headers, key → value
}
```

The `headers` sub-table allows setting arbitrary RFC 2822 headers beyond the
standard ones.  For example:

```lua
on_reply = function(profile, parent, mail)
  mail.headers = mail.headers or {}
  mail.headers["X-Mailer"] = "lorebird"
  mail.headers["Reply-To"] = "list@example.com"
  -- no return needed; modifications are picked up automatically
end
```

These headers are included in the RFC 2822 output by `mail_to_rfc2822()`.

---

## Rust configuration types

```rust
use std::collections::HashMap;
use std::path::PathBuf;

/// Global user identity (name + email) used as defaults for profiles
/// that don't define their own.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UserInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// A saved view — named query displayed in the sidebar.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ViewConfig {
    pub label: String,
    pub query: String,
}

/// Data portion of a profile (deserialised from Lua via serde).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProfileData {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    pub maildir: String,
    #[serde(default)]
    pub views: Vec<ViewConfig>,
}

/// The full configuration data, deserialised from the Lua `config` table.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub user: Option<UserInfo>,
    pub profiles: HashMap<String, ProfileData>,
}

/// Resolved profile with inherited user info filled in.
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub label: String,
    pub name: String,
    pub email: String,
    pub maildir: PathBuf,
    pub views: Vec<ViewConfig>,
    pub smtp: Option<SmtpConfig>,  // per-profile SMTP config (if any)
}
```

**Resolution logic** for `name` and `email`:

```rust
fn resolve(profile: &ProfileData, global: &Option<UserInfo>) -> (&str, &str) {
    let name  = profile.name.as_deref()
                .or(global.as_ref().and_then(|u| u.name.as_deref()))
                .unwrap_or("Anonymous");
    let email = profile.email.as_deref()
                .or(global.as_ref().and_then(|u| u.email.as_deref()))
                .unwrap_or("unknown@localhost");
    (name, email)
}
```

Hooks (`on_fetch`, `on_reply`, `on_send`) are stored as `mlua::Function`
separately from the data struct — they cannot be serde-deserialised.

---

## Compose types (Rust)

In addition to the configuration types above, lorebird defines two
compose-related structs in `lorebird_core::compose`:

```rust
/// A mail message — used for both parent (read-only) and compose (mutable).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Mail {
    pub from: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub date: Option<String>,       // RFC 2822 Date; None = auto-generate at send time
    pub message_id: Option<String>, // e.g. "<unique@host>"; None = auto-generate
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub body_text: String,
    pub headers: HashMap<String, String>,  // arbitrary RFC 2822 headers
}
```

`Mail::new_reply(parent, profile_name, profile_email)` builds a
pre-filled reply following standard mailing-list conventions: To is set
to `parent.from` (the person being replied to), Cc merges `parent.to +
parent.cc` minus self (deduplicated).  Prefix "Re:", chain References,
set In-Reply-To, quote body).  The Lua
`on_reply` hook can modify this before the compose window opens.

`ComposeMail::to_rfc2822()` serialises the mail to RFC 2822 format.  This
is what `mail_to_rfc2822()` calls under the hood.  In addition to the
explicit `ComposeMail` fields, the output always includes:

| Header | How it's generated |
|--------|--------------------|
| `Date` | `mail.date` if set, otherwise the current local time in RFC 2822 format |
| `Message-ID` | `mail.message_id` if set, otherwise `<YYYYMMDDHHMMSS.COUNTER-USER@DOMAIN>` (sender email-derived, *git send-email* style) |
| `MIME-Version` | `1.0` (unless overridden in `mail.headers`) |
| `Content-Type` | `text/plain; charset=utf-8` (unless overridden in `mail.headers`) |
| `X-Mailer` | `lorebird` (unless overridden in `mail.headers`) |

If the user sets one of these keys in `mail.headers`, the default is
suppressed and the user's value is used instead.

---

## Compose window

When the user clicks **Reply** (via Ctrl+R or right-click context menu),
lorebird opens a compose window:

- **Trigger**: Ctrl+R keyboard shortcut, or right-click → Reply on any
  message in the thread list
- **Header fields**: editable `From`, `To`, `Cc`, `Bcc`, `Subject` entries
- **Body**: GTK SourceView widget with line numbers, syntax highlighting
  using the `diff` language (for `[PATCH]` mails showing `---` hunk markers)
- **Action bar**: **Send** / **Discard** buttons in the header bar

The compose window receives the `mail` table (default-filled or hook-modified)
and lets the user edit everything.  On Send, lorebird:

1. Collects all header fields and body text from the compose window
2. Builds a `ComposeMail` struct
3. Calls `on_send(profile, mail)` via the Lua thread
4. The hook can use `mail_to_rfc2822(mail)` and `write_tmpfile(content)` to
   format and deliver the message
5. On success, the compose window closes; on failure, an error is shown

GTK SourceView language definitions for `diff` syntax are available out of
the box, providing coloured hunks for patch review.

---

## Future: async fetch

The initial implementation calls `sh()` synchronously, which blocks the
UI.  For long-running fetches (large mailing lists), this should be replaced
with an async model:

1. UI shows a spinner / progress bar in the status area
2. `on_fetch` runs in a background thread
3. On completion, a glib `idle_add` callback updates the UI and triggers
   indexing

This is a post-v0.1 improvement.  The hook interface (`on_fetch` returns
truthy/falsy) stays the same.

---

## Summary of data flow

```
┌─────────────────────────────────────────────────────────────┐
│  Lua config                                                 │
│  config = {                                                 │
│    user = { name, email },                                  │
│    profiles = {                                             │
│      ["label"] = { maildir, views, on_fetch },              │
│    },                                                       │
│    on_reply, on_send,                                       │
│  }                                                          │
└──────────────────┬──────────────────────────────────────────┘
                   │ mlua::serde for data
                   │ mlua::Function for hooks
                   ▼
┌──────────────────────────────────────────────────────────────┐
│  Rust: AppConfig + hooks                                    │
│                                                              │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────────┐ │
│  │ ProfileData  │   │ mlua::Func   │   │ ResolvedProfile  │ │
│  │ (serde)      │   │ (on_fetch,   │   │ (name/email     │ │
│  │              │   │  on_reply,   │   │  resolved from   │ │
│  │              │   │  on_send)   │   │  global defaults)│ │
│  └──────────────┘   └──────────────┘   └──────────────────┘ │
└──────────────────────────────────────────────────────────────┘
                   │
                   ▼  (profile selected in sidebar)
┌──────────────────────────────────────────────────────────────┐
│  Index + display                                             │
│                                                              │
│  1. on_fetch(label, maildir) → truthy?                                │
│  2. index_maildir(conn, &profile.maildir)                    │
│  3. SELECT * FROM mail_ndx → JWZ thread_messages()           │
│  4. Populate ColumnView with ThreadNode tree                 │
│  5. Apply view.query (if any) → filter visible threads       │
└──────────────────────────────────────────────────────────────┘
                   │
                   ▼  (user clicks Reply: Ctrl+R or right-click)
┌──────────────────────────────────────────────────────────────┐
│  Reply flow                                                  │
│                                                              │
│  1. Rust builds pre-filled Mail (From = profile, To = parent.from,     │
│     Cc = parent.to + parent.cc − self, Subject, In-Reply-To,    │
│     Subject, In-Reply-To, References, quoted body)           │
│  2. Call on_reply(profile, parent, mail) if hook exists      │
│     → may modify the mail table or return nil               │
│  3. Open compose window with (possibly modified) mail         │
│  4. User edits in compose window                             │
│  5. On Send: call on_send(profile, mail)                     │
│     → hook uses mail_to_rfc2822(mail) + write_tmpfile()     │
│     → and/or sh() to pipe to sendmail                       │
└──────────────────────────────────────────────────────────────┘
```