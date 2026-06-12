# Lorefetch Module — Design Specification

## Purpose

Fetch mail from lore.kernel.org (or other public-inbox instances) into a
maildir, with streaming processing, incremental queries, and automatic
Anubis challenge solving.

The crate provides:

- A library (`lorebird-lorefetch`) for in-process use from the Lua thread
- A CLI tool (`tools/lorefetch`) replicating the original Go program's flags

---

## Workspace layout

```
Cargo.toml                          # members include "crates/lorebird-lorefetch", "tools/lorefetch"

crates/lorebird-lorefetch/
  Cargo.toml
  src/
    lib.rs                           # all code in one file

tools/lorefetch/
  Cargo.toml
  src/
    main.rs                          # clap-based CLI
```

---

## Dependencies

```toml
# crates/lorebird-lorefetch/Cargo.toml
[dependencies]
ureq = { version = "3", features = ["json", "cookies", "gzip", "brotli", "charset"] }
sha1 = "0.10"
sha2 = "0.10"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
mail-parser = "0.10"
thiserror = "2"
flate2 = "1"                        # manual gzip decompression (see below)

[dev-dependencies]
tempfile = "3"
```

> **No `tokio`, no `reqwest`, no `regex`.** `ureq` is synchronous — no
> async runtime needed. `flate2` handles gzip decompression because
> lore.kernel.org sends `Content-Type: application/gzip` (not
> `Content-Encoding: gzip`), so ureq's auto-decompression doesn't
> trigger. We detect the `0x1f 0x8b` magic bytes and wrap in
> `flate2::read::GzDecoder` manually.

---

## Constants

| Constant | Value | Notes |
|---|---|---|
| `BASE_URL` | `"https://lore.kernel.org"` | Configurable on `LoreClient` |
| `USER_AGENT` | `"Lorefetch/1.x (…)"` | Matches Go version |
| `READ_TIMEOUT_SECS` | `30` | Read-stall timeout: no bytes for 30 s = abort |
| `CACHE_VERSION` | `2` | Cache format version; mismatch → delete & rebuild |
| `CACHE_FILENAME` | `".lorefetch-cache.json"` | Stored inside the maildir |
| `MAIL_FILE_MODE` | `0o644` | Unix mode for written mail files |
| `MAILDIR_MODE` | `0o755` | Unix mode for created directories |

---

## Error type

```rust
#[derive(Debug, thiserror::Error)]
pub enum LoreError {
    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error("Anubis challenge: {0}")]
    Anubis(String),

    #[error("Mbox is empty or contains no messages")]
    EmptyMbox,

    #[error("Message has no Message-ID header (message {index})")]
    MissingMessageId { index: usize },

    #[error("Maildir: {0}")]
    Maildir(String),

    #[error("Cache I/O: {0}")]
    CacheIo(#[from] std::io::Error),

    #[error("Cache format: {0}")]
    CacheFormat(String),

    #[error("Response is not mbox format")]
    NotMbox,

    #[error("Read timeout: no data received for {0}s")]
    ReadTimeout(u64),
}
```

---

## Result type

```rust
pub struct FetchResult {
    pub new_messages: usize,    // written to maildir
    pub total_messages: usize,  // seen in mbox (new + cached)
    pub timed_out: bool,        // partial fetch due to stall
}
```

---

## Architecture: streaming pipeline

```
HTTP POST → response bytes
   │
   ▼
detect_content(first bytes)
   │
   ├── 0x1f 0x8b (gzip) ──► make_reader() wraps in GzDecoder
   │                            │
   ├── "From " (raw mbox) ──► make_reader() wraps in Cursor
   │                            │
   └── "<" or "<!DOCTYPE" ──► Anubis challenge detected
                                  (buffer full HTML, solve, retry)
                                  │
                                  ▼ (retry response goes through
                                    same detection → mbox or error)

make_reader() returns Box<dyn Read + Send>
   │
   ▼
MboxParser<R: Read>
   │  reads one line at a time
   │  yields one complete message per next_message() call
   ▼
per-message processing:
   SHA-1(Message-ID) → cache check → write to maildir (or skip)
   parse Date: header → track newest_date
   │
   ▼
update cache with last_date, save atomically
```

The entire pipeline is streaming.  No stage buffers the full mbox in
memory.  The `MboxParser` reads lines one at a time from a `BufReader`
and yields complete messages as `Vec<u8>`.

### Content detection from first bytes

| First bytes | Type | Action |
|---|---|---|
| `0x1f 0x8b` | Gzip | Wrap in `GzDecoder`, stream through `MboxParser` |
| `From ` | Raw mbox | Stream through `MboxParser` directly |
| `<` or `<!DOCTYPE` | HTML | Anubis challenge or error; buffer, solve, retry |

### Anubis challenge flow

Anubis challenge pages are small (~5 KB) and must be buffered in full
to extract the `<script id="anubis_challenge">` JSON, solve the PoW,
and submit the solution.  After solving, the retry response goes
through the same content detection and streams normally.

### Read-stall timeout

`ureq::Config::builder().timeout_recv_body(Some(Duration::from_secs(30)))`
triggers if no bytes arrive for 30 seconds during the response body
read.  This is a *stall* timeout, not a total timeout — a 50 MB
response that streams steadily for 3 minutes never gets killed.

On timeout, all messages already processed are preserved, the cache
is updated with the `newest_date` seen so far, and `FetchResult {
timed_out: true }` is returned.

---

## Streaming mbox parser

```rust
struct MboxParser<R: Read> {
    reader: BufReader<R>,    // 64 KB read buffer
    line_buf: Vec<u8>,       // current line accumulator
    msg_buf: Vec<u8>,        // current message accumulator
    msg_index: usize,        // message counter for error reporting
}

impl<R: Read> MboxParser<R> {
    fn new(reader: R) -> Self;
    fn next_message(&mut self) -> Result<Option<(usize, Vec<u8>)>, LoreError>;
}
```

`next_message()` reads lines from the `BufReader` and accumulates them
into `msg_buf`.  When a `From ` separator line is encountered (and
the buffer is non-empty), the accumulated message is yielded.  At EOF,
any remaining buffer content is yielded as the final message.

### mboxrd `>From ` escaping

Lines starting with `>+From ` have one leading `>` stripped
(un-escaping).  lore.kernel.org serves mboxrd format.

---

## Incremental queries — per-query `last_date`

### Cache format

```json
{
  "version": 2,
  "cache": ["sha1hex1", "sha1hex2"],
  "queries": {
    "s:linux-kernel": { "last_date": "2025-06-08" },
    "s:linux-raid":    { "last_date": "2025-05-01" }
  }
}
```

| Field | Purpose |
|-------|---------|
| `cache` | `HashSet<String>` — SHA-1 hex filenames already in the maildir (union across all queries) |
| `queries` | Per-query incremental state, keyed by the original query string (without any injected `dt:`) |
| `queries[q].last_date` | Newest `Date:` header seen for this query, in `YYYY-MM-DD` format |

**No backwards compatibility.** If the cache file fails to deserialize
or has a version mismatch, the old file is deleted and a new cache is
built from scratch by scanning `new/` and `cur/`.

### Delta queries

On each fetch, look up the current query in `queries`:

- **Found**: inject `dt:` with `last_date − 1 day` as the lower bound.
- **Not found**: fetch the full query (no `dt:` injection).

```
First fetch:  s:linux-kernel
Second fetch: s:linux-kernel dt:20250607..   (last_date was 2025-06-08)
```

The 1-day overlap (`20250607` instead of `20250608`) prevents gaps
from timezone skew, delayed server indexing, or clock differences.
Overlap messages are skipped instantly via SHA-1 cache lookup.

### Existing `dt:` in the query

If the user's query already contains `dt:`, the lower bound is
replaced: `dt:20240101..1.month.ago` with `last_date = 2025-03-15`
becomes `dt:20250314..1.month.ago`.  The upper bound is preserved.

### Query changes

Switching queries does **not** rebuild the cache.  We simply do a full
fetch (no `dt:` injection) and create a new entry in `queries`.
Switching back to a previous query is still incremental — its
`last_date` is preserved.

---

## Maildir cache — naming convention

**Each mail file is named by the hex-encoded SHA-1 hash of its
`Message-ID` header value** (including angle brackets).  This matches
the Go program exactly.

Files are written to `new/` with mode `0o644`.  Maildir directories
are created with mode `0o755`.

---

## Date parsing for `last_date`

The `Date:` header is parsed by `mail_parser`'s `.date()` method,
then converted to RFC 2822 format and parsed into `YYYY-MM-DD` by
`parse_date_string()`.  This handles common formats like
`Fri, 06 Jun 2025 14:32:00 +0000` and `06 Jun 2025 14:32:00 +0000`.

---

## Anubis challenge solving

Detection uses simple string matching (no regex).  The challenge JSON
is extracted from `<script id="anubis_challenge">` using string ops
and parsed with `serde_json` into typed structs:

```rust
#[derive(Deserialize)]
struct AnubisChallengeJson {
    challenge: String,
    rules: AnubisRulesJson,
}

#[derive(Deserialize)]
struct AnubisRulesJson {
    algorithm: String,
    difficulty: usize,
    report_as: usize,
}
```

Solving uses brute-force SHA-256 proof-of-work, identical to the Go
version.  The solution is submitted via GET to the Anubis pass-challenge
endpoint, and the auth cookie is preserved in ureq's cookie jar for
the retry request.

---

## Public API

```rust
/// Result of a fetch operation.
pub struct FetchResult {
    pub new_messages: usize,
    pub total_messages: usize,
    pub timed_out: bool,
}

/// Fetch mail into a maildir.  Streaming pipeline with incremental queries.
pub fn fetch_to_maildir(
    query: &str,
    list: Option<&str>,
    maildir: &Path,
    verbose: bool,
) -> Result<FetchResult, LoreError>;

/// HTTP client for lore.kernel.org (or other public-inbox instances).
pub struct LoreClient { /* opaque */ }

impl LoreClient {
    pub fn new() -> Self;
    pub fn with_base_url(url: &str) -> Self;
    pub fn verbose(mut self, v: bool) -> Self;

    /// Fetch raw mbox content and write to a file (for CLI --mbox flag).
    pub fn fetch_mbox_to_file(
        &self, query: &str, list: Option<&str>, path: &Path
    ) -> Result<usize, LoreError>;
}
```

---

## CLI tool — `tools/lorefetch`

```rust
#[derive(Parser)]
struct Args {
    #[arg(short, long)]              query: String,
    #[arg(short, long)]              list: Option<String>,
    #[arg(short, long)]              maildir: Option<PathBuf>,
    #[arg(long)]                      mbox: Option<PathBuf>,
    #[arg(short = 'v', long = "verbose", action = Count)]
                                      verbose: u8,
}
```

- `--maildir`: calls `fetch_to_maildir()` — streams, dedupes, writes `new/`
- `--mbox`: calls `LoreClient::fetch_mbox_to_file()` — writes raw mbox to file
- Prints `"{new} new of {total} total messages fetched"` (or timeout warning)

---

## Lua integration — `lorefetch(maildir, query)`

```lua
on_fetch = function(label, maildir)
    local result = lorefetch(maildir, "s:linux-kernel")
    if result.timed_out then
        print("partial fetch: " .. result.new .. " new, timed out")
    end
    return result.ok
end
```

Returns a table:
```lua
{ ok = true,  count = N, new = M, timed_out = false }
{ ok = true,  count = N, new = M, timed_out = true }  -- partial fetch
{ ok = false, error = "..." }
```

---

## Design decisions

| Decision | Rationale |
|---|---|
| Single `lib.rs` | Go program is one 709-line file. Sections keep it navigable. |
| SHA-1 for filenames | Faithful to Go. SHA-1 is for naming, not security. |
| Streaming pipeline | No stage buffers the full mbox. Reduces peak memory and enables per-message stall detection. |
| Read-stall timeout (30 s) | Detects genuine stalls (no bytes for 30 s) without killing long transfers. |
| `ureq` (synchronous) | Matches the Lua thread's blocking model. No async runtime needed. |
| `flate2` for gzip | Server sends `Content-Type: application/gzip`, not `Content-Encoding: gzip`. ureq's feature flag doesn't trigger. Manual `0x1f 0x8b` detection + `GzDecoder`. |
| Per-query `last_date` | Switching queries doesn't lose incremental state. 1-day overlap prevents gaps. |
| No backwards compat for cache | v1 caches are deleted and rebuilt. No migration code. |
| `mail_parser` crate reuse | Already a dependency of `lorebird-core`. |
| `serde` for Anubis JSON | `AnubisChallengeJson` / `AnubisRulesJson` structs + `serde_json::from_str()`. |
| mboxrd `>From ` un-escaping | `unescape_mboxrd()` strips one `>` from `>+From ` lines. |

---

## Differences from the Go program

| Go lorefetch | Rust `lorebird-lorefetch` | Reason |
|---|---|---|
| Cache format: gob binary | JSON with per-query `last_date` | gob is Go-specific; JSON is debuggable |
| Cache filename: `.lorefetch-cache.gob` | `.lorefetch-cache.json` | Different extension; same key format |
| Downloads entire response into memory | Streams through `MboxParser` one message at a time | Reduces peak memory; enables stall detection |
| No incremental query support | Per-query `last_date` with `dt:` injection | Major feature addition |
| `log.Printf` for diagnostics | `eprintln!` in CLI, `FetchResult` in library | Library returns structured results |
| Panic on missing Message-ID | Returns `LoreError::MissingMessageId` | Rust error handling |
| Total HTTP timeout | Read-stall timeout (30 s no-data) | Doesn't kill long transfers |

---

## Test plan

1. **Mbox parser.** Known mbox text → correct message count and content.
2. **Streaming MboxParser.** Same tests via `MboxParser<R: Read>` from a `Cursor`.
3. **SHA-1 naming.** Known `Message-ID` → known hex filename. Cross-check with Go.
4. **Cache init.** Temp maildir with files in `new/` and `cur/` (with `:2,S`) → correct cache.
5. **Cache version mismatch.** Wrong version → delete & rebuild from maildir.
6. **Anubis solver.** Known challenge + difficulty → known nonce/hash.
7. **Date parsing.** RFC 2822 dates → `YYYY-MM-DD` strings.
8. **Incremental queries.** `build_incremental_query(query, last_date)` → correct `dt:` ranges.
9. **Day subtraction.** Boundary cases (month, year, leap year).
10. **Content detection.** `detect_content()` correctly identifies gzip, HTML, raw mbox.
11. **Gzip round-trip.** Compress test data with `GzEncoder`, decompress with `make_reader()`.
12. **End-to-end.** Live test against lore.kernel.org.