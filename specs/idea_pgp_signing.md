# PGP Signing via `on_send` Hook

This is not implemented. It is a design note showing how PGP signing and encryption
can be achieved using the existing `on_send` hook, `mail_to_rfc2822()`, `send_smtp()`,
and `sh()` with stdin support (**now implemented**).

---

## The insight

The `on_send` hook receives a **mail object** (Lua table), not a raw RFC 2822 string.

```lua
{
  from = "\"Alice\" <alice@example.com>",
  to = "bob@example.com",
  cc = "", bcc = "",
  subject = "Re: patch series v2",
  date = "Sun, 8 Jun 2026 12:00:00 +0000",
  message_id = "<lorebird.1234@example.com>",
  in_reply_to = "<original@example.com>",
  references = "<original@example.com>",
  body_text = "Hi Bob,\n\nLooks good to me.\n\n-- \nAlice",
  headers = {
    ["X-Mailer"] = "lorebird",
  }
}
```

And `mail_to_rfc2822(mail)` serializes that table into a valid RFC 2822 string.

This means the hook can modify **structured fields** — change `Content-Type`, replace
`body_text`, add headers — without string-splitting or regex-parsing a monolithic
RFC 2822 blob. Then serialize and send the modified object.

---

## PGP/MIME signing (RFC 3156)

### How it works

1. The hook receives the mail table
2. Sign `mail.body_text` with `gpg --detach-sign`
3. Replace `mail.body_text` with the `multipart/signed` body
4. Set the `Content-Type` header to `multipart/signed`
5. Call `mail_to_rfc2822(mail)` → `send_smtp(result)`

### Example Lua code

```lua
on_send = function(label, mail)
    local boundary = "lorebird_pgp_" .. os.time()

    -- Sign the body
    local sig = sh({"gpg", "--armor", "--detach-sign"}, { stdin = mail.body_text })

    -- Restructure as multipart/signed
    mail.headers["Content-Type"] =
        'multipart/signed; protocol="application/pgp-signature"; boundary="' .. boundary .. '"'

    mail.body_text =
          "--" .. boundary .. "\r\n"
       .. mail.body_text .. "\r\n"
       .. "--" .. boundary .. "\r\n"
       .. "Content-Type: application/pgp-signature\r\n"
       .. "\r\n"
       .. sig .. "\r\n"
       .. "--" .. boundary .. "--\r\n"

    -- Serialize and send
    send_smtp(mail_to_rfc2822(mail))
end
```

### What happens at each step

| Step | Before | After |
|------|--------|-------|
| `mail.body_text` | `"Looks good.\n"` | `"--boundary\r\nLooks good.\r\n--boundary\r\nContent-Type: ...\r\n\r\n-----PGP SIG-----\r\n--boundary--\r\n"` |
| `mail.headers["Content-Type"]` | absent / `text/plain` | `multipart/signed; protocol="application/pgp-signature"; boundary="..."` |
| Final output of `mail_to_rfc2822(mail)` | flat RFC 2822 | RFC 2822 with multipart/signed MIME structure |

---

## PGP/MIME encryption (RFC 3156)

Same pattern, but `gpg --encrypt` produces an encrypted block. The structure is
`multipart/encrypted` with two parts: a `application/pgp-encrypted` version part
(always `Version: 1`) and an `application/octet-stream` part containing the encrypted
data.

```lua
on_send = function(label, mail)
    local recipient = mail.to:match("<(.-)>") or mail.to
    local boundary = "lorebird_pgp_" .. os.time()

    -- Encrypt the full RFC 2822 body (without transport headers)
    local plain_rfc = mail_to_rfc2822(mail)
    local encrypted = sh({"gpg", "--armor", "--encrypt", "-r", recipient}, { stdin = plain_rfc })

    -- Restructure as multipart/encrypted
    mail.headers["Content-Type"] =
        'multipart/encrypted; protocol="application/pgp-encrypted"; boundary="' .. boundary .. '"'

    mail.body_text =
          "--" .. boundary .. "\r\n"
       .. "Content-Type: application/pgp-encrypted\r\n"
       .. "\r\n"
       .. "Version: 1\r\n"
       .. "--" .. boundary .. "\r\n"
       .. "Content-Type: application/octet-stream\r\n"
       .. "\r\n"
       .. encrypted .. "\r\n"
       .. "--" .. boundary .. "--\r\n"

    send_smtp(mail_to_rfc2822(mail))
end
```

**Note**: PGP/MIME encryption encrypts the *entire* inner message (headers + body),
not just the body. So we serialize with `mail_to_rfc2822(mail)` first, encrypt that,
then restructure the outer message as `multipart/encrypted`.

---

## Combined sign+encrypt

Chain both operations: sign first (to get the detached signature embedded in the
inner message), then encrypt the signed message.

```lua
on_send = function(label, mail)
    -- 1. Sign the body
    local sig = sh({"gpg", "--armor", "--detach-sign"}, { stdin = mail.body_text })
    local sign_boundary = "lorebird_sign_" .. os.time()
    mail.headers["Content-Type"] =
        'multipart/signed; protocol="application/pgp-signature"; boundary="' .. sign_boundary .. '"'
    mail.body_text =
          "--" .. sign_boundary .. "\r\n"
       .. mail.body_text .. "\r\n"
       .. "--" .. sign_boundary .. "\r\n"
       .. "Content-Type: application/pgp-signature\r\n\r\n"
       .. sig .. "\r\n"
       .. "--" .. sign_boundary .. "--\r\n"

    -- 2. Encrypt the signed message
    local recipient = mail.to:match("<(.-)>") or mail.to
    local plain = mail_to_rfc2822(mail)
    local encrypted = sh({"gpg", "--armor", "--encrypt", "-r", recipient}, { stdin = plain })
    local enc_boundary = "lorebird_enc_" .. os.time()
    mail.headers["Content-Type"] =
        'multipart/encrypted; protocol="application/pgp-encrypted"; boundary="' .. enc_boundary .. '"'
    mail.body_text =
          "--" .. enc_boundary .. "\r\n"
       .. "Content-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n"
       .. "--" .. enc_boundary .. "\r\n"
       .. "Content-Type: application/octet-stream\r\n\r\n"
       .. encrypted .. "\r\n"
       .. "--" .. enc_boundary .. "--\r\n"

    send_smtp(mail_to_rfc2822(mail))
end
```

---

## What needs to exist in Rust

~~Only one thing: **`sh()` must support piping stdin to the child process.**~~

**✅ Implemented.** `sh()` now accepts an optional `opts` table with `stdin`, `stdin_file`, and `env`.

Current signature:

```lua
sh(cmd)                                -- runs cmd, inherits stdin from lorebird process
sh(cmd, { stdin = "data to pipe" })     -- pipe string to stdin
sh(cmd, { stdin_file = "/path/to/f" })  -- pipe file to stdin
sh(cmd, { env = { KEY = "value" } })   -- extra environment variables
```

The `opts.stdin_file` from the original spec (pipe from a file) is a separate
extension but follows the same pattern — open the file, set `Stdio::from(file)`.

---

## Why this is the right design

### No Rust PGP crate needed

PGP implementations are complex (sequoia, rpgp, etc.). Shelling out to `gpg`
uses the user's existing keyring, agent, and trust database. No key management
in lorebird, no Rust crypto dependency surface.

### The mail object is the key enabler

If `on_send` received a raw RFC 2822 string (as many MUAs do), PGP signing would
require:

1. Find the header/body boundary (search for `\n\n`)
2. Modify the `Content-Type` header in place (regex or line-by-line)
3. Replace the body with the multipart structure
4. Reassemble

Error-prone, fragile under edge cases (folded headers, charset declarations, etc.).
With the table representation:

```lua
mail.headers["Content-Type"] = "multipart/signed; ..."
mail.body_text = multipart_body
```

Two assignments. `mail_to_rfc2822()` handles all the formatting.

### It generalizes

The same pattern works for any message transformation that a hook might want:

| Use case | How |
|----------|-----|
| PGP sign | Sign body, restructure as multipart/signed |
| PGP encrypt | Encrypt full message, restructure as multipart/encrypted |
| DKIM sign | Shell out to `openssl`, add `DKIM-Signature` header |
| Add custom headers | `mail.headers["X-Custom"] = "value"` |
| Rewrite body | `mail.body_text = transformed_body` |
| Attach files | Extend body as `multipart/mixed` with file contents |

None of these require Rust code changes.

---

## Comparison with how other MUAs do it

| MUA | PGP approach | Our approach |
|-----|-------------|--------------|
| mutt | Built-in gpg integration, `$pgp_sign_as` config | Lua hook, shell out to gpg |
| aerc | Built-in, or `aerc-pgp` filter | Lua hook, shell out to gpg |
| Thunderbird | Built-in OpenPGP (RNP library) | Lua hook, shell out to gpg |
| NeoMutt + gpg | Built-in, config-driven | Lua hook, shell out to gpg |

Our approach trades built-in convenience for **extensibility**. A Lua hook can do
anything — PGP, S/MIME, DKIM, custom signing schemes — without Rust changes. The
cost is that users must write (or cargo-cult) a ~15-line Lua function. This could
be mitigated by shipping example hooks in `/share/lorebird/examples/`.

---

## Future: helper library

If PGP signing via hooks becomes common, we could ship a `pgp.lua` helper module
that provides:

```lua
local pgp = require("lorebird.pgp")

on_send = function(label, mail)
    pgp.sign(mail)         -- modifies mail in place
    send_smtp(mail_to_rfc2822(mail))
end
```

But that's a convenience wrapper, not a change in architecture. The hook pattern
is the foundation.