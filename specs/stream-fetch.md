# Stream-based lore.kernel.org fetch with incremental queries

## Implementation status

| Feature | Status |
|---|---|
| Read stall timeout (`timeout_recv_body` via ureq) | ✅ Done |
| Per-query `last_date` cache (v2 format) | ✅ Done |
| `build_incremental_query()` with 1-day overlap | ✅ Done |
| `dt:` lower-bound injection / replacement | ✅ Done |
| Cache version mismatch → delete & rebuild | ✅ Done |
| `parse_date_string()` for `Date:` header extraction | ✅ Done |
| `FetchResult { new_messages, total_messages, timed_out }` | ✅ Done |
| Content detection from first bytes (`detect_content`) | ✅ Done |
| `MboxParser<R: Read>` streaming parser struct | ✅ Done |
| mboxrd `>From ` un-escaping (`unescape_mboxrd`) | ✅ Done |
| End-to-end streaming: response → GzDecoder → MboxParser → per-message processing | ✅ Done |
| Partial fetch on timeout: save what we have, update cache, return `timed_out: true` | ✅ Done |
| Anubis challenge detection and solving | ✅ Done |
| Buffered `parse_mbox()` removed | ✅ Done (replaced by streaming MboxParser) |

The streaming pipeline is fully implemented.  Messages are processed
one at a time as they arrive from the wire, never buffered in full.
The `MboxParser` yields each message individually, and the
`MboxParser::next_message()` method propagates `ReadTimeout` errors
so partial fetches are saved gracefully.

The only buffered path is the Anubis challenge branch: challenge
pages are small HTML (~5 KB) that must be fully buffered to extract
the PoW challenge JSON.  After solving, the retry response streams
normally through `MboxParser`.

## Problem (solved)

The current lorefetch implementation downloads the entire gzipped mbox
response into memory, decompresses it wholesale, then parses all messages
at once.  This has three issues:

1. **HTTP timeouts are meaningless.** A total timeout of 60 seconds
   either kills legitimate long responses (large queries can yield
   hundreds of megabytes) or lets genuine stalls run forever.
2. **No incremental updates.** Every fetch re-downloads the entire
   history for the query.  For a list like `linux-kernel`, this is
   millions of messages every time.
3. **Memory pressure.** The full response body, decompressed mbox, and
   parsed messages are all in memory simultaneously.

## Architecture

lei (the reference client) solves this by streaming the entire pipeline:
curl → gunzip → mbox parse → dedup → write.  Each stage pulls from
the previous one, providing natural backpressure.  No component ever
holds the full result set.

We adopt the same model.  The fetch pipeline becomes:

```
HTTP response body (ureq Read trait)
   │
   ▼
peek first 2 bytes ─────────────────────────────────────┐
   │                                                      │
   ├── 0x1f 0x8b (gzip magic)                             │
   │      │                                                │
   │      ▼                                                │
   │   GzDecoder (streaming decompression)                 │
   │      │                                                │
   │      ▼                                                │
   │   peek decompressed head (first ~4 KB)                │
   │      │                                                │
   │      ├── starts with "From " → stream mbox            │
   │      └── starts with "<"      → Anubis challenge      │
   │                                   (buffer, solve, retry)
   │                                                        │
   ├── "From " (raw mbox)                                   │
   │      │                                                  │
   │      ▼                                                  │
   │   stream mbox directly                                  │
   │                                                          │
   └── "<" or "<!DOCTYPE" (HTML)                              │
          │                                                    │
          ▼                                                    │
       Anubis challenge page                                   │
       (buffer entire HTML, parse, solve, retry)               │
          │                                                    │
          ▼  (retry response goes through same detection)       │
          └────────────────────────────────────────────────────┘

MboxParser (yields one complete RFC 2822 message at a time)
   │
   ▼
per-message processing:
   SHA-1 → cache check → write to maildir (or skip)
   track newest Date: header
   │
   ▼
update cache with last_date
```

## Content detection from the stream

Rather than relying on HTTP headers (which lore.kernel.org sets
incorrectly — `Content-Type: application/gzip` instead of
`Content-Encoding: gzip`), we detect the response type from the
first bytes of the body:

| First bytes | Meaning | Action |
|---|---|---|
| `0x1f 0x8b` | Gzip stream | Wrap in `GzDecoder`, then check decompressed head |
| `From ` | Raw mbox | Stream parse directly |
| `<` or `<!DOCTYPE` | HTML page | Anubis challenge or error page; buffer and handle |

After wrapping in `GzDecoder`, we peek at the first ~4 KB of
decompressed output:

| Decompressed head starts with | Meaning | Action |
|---|---|---|
| `From ` | Mbox content | Continue streaming |
| `<` | HTML (Anubis challenge or error) | Buffer full response, handle accordingly |

This two-stage detection means:

- **99% of requests** (mbox data, gzipped or not) go straight to
  streaming with zero buffering beyond the `BufReader`.
- **Anubis challenges** are detected early (the decompressed head
  starts with `<` instead of `From `), and the full HTML is buffered
  for challenge parsing.  But Anubis challenges are small (a few KB)
  and rare (once per session, typically).
- **Error responses** (502, etc.) that come as gzipped HTML are also
  caught at the decompressed head check.

### Anubis challenge flow

The Anubis path is inherently buffered — you need the full HTML to
extract the `<script id="anubis_challenge">` JSON, solve the PoW, and
submit the solution.  But this is fine: challenge pages are small
(~5 KB), and solving takes at most a few seconds.  After solving, the
retry request goes through the same content detection and streams
normally.

```
POST request
   │
   ▼
peek first 2 bytes
   │
   └── (gzip or raw, starts with "<")
       │
       ▼
   buffer full HTML response (~5 KB)
       │
       ▼
   extract Anubis challenge JSON
       │
       ▼
   solve SHA-256 PoW
       │
       ▼
   submit solution (GET with cookie)
       │
       ▼
   retry original POST
       │
       ▼
   (response now starts with "From " → stream mbox)
```

When in the Anubis branch, the read-stall timeout still applies: if
the HTML response stalls mid-download, we abort and report an error.
But in practice, challenge pages are tiny and arrive quickly.

## Timeouts — stall detection, not total time

```rust
let agent = ureq::config::Config::builder()
    .timeout_recv_body(Some(Duration::from_secs(30)))
    .build()
    .into();
```

A streaming mbox from lore.kernel.org delivers bytes continuously
during a large result.  If nothing arrives for ~30 seconds, that is a
genuine stall (broken TCP, hung server, network partition).  But a
50 MB response that takes 3 minutes of steady streaming never gets
killed.

The `READ_TIMEOUT_SECS = 30` constant applies per `read()` syscall:
the kernel returns `ETIMEDOUT` or ureq raises `Error::Timeout` if no
data arrives within the window.  This is qualitatively different from
a total request timeout.

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
| `cache` | `HashSet<String>` — SHA-1 hex filenames already in the maildir |
| `queries` | Per-query incremental state — key is the query string (without any injected `dt:`) |
| `queries[q].last_date` | `Date:` header of the newest message seen for this query, in `YYYY-MM-DD` format |

The `cache` set is the union of all messages ever fetched for this
maildir, regardless of which query produced them.  When queries overlap
(e.g. `s:linux-kernel` and `s:linux-kernel+author:torvalds`),
duplicate messages are detected by SHA-1 and skipped — the server
still sent them, but we never write them to disk twice.

Per-query tracking means switching between queries doesn't lose
incremental state.  If a user fetches `s:linux-kernel` (tracking
`last_date`), then switches to `s:linux-raid` (first fetch is full),
then switches back to `s:linux-kernel`, the second kernel fetch is
still incremental — it picks up from the stored `last_date`.

Whether users keep distinct maildirs per profile or throw everything
into a single directory, the cache handles it correctly.

### Cache version handling

There is no backwards compatibility code.  If the cache file fails to
deserialize or has a version mismatch, the old file is deleted and a
new cache is built from scratch by scanning `new/` and `cur/` in the
maildir.  No one ships this code yet; no migration path is needed.

### Delta queries

On each fetch, look up the current query in `queries`:

- **Found**: inject `dt:` with `last_date − 1 day` as the lower bound.
- **Not found**: fetch the full query (no `dt:` injection).

```
# First-ever fetch for this query (no entry in queries):
s:linux-kernel

# Second fetch (last_date = 2025-06-03):
s:linux-kernel dt:20250602..   # 1-day overlap

# Third fetch (last_date = 2025-06-08):
s:linux-kernel dt:20250607..   # again 1-day overlap
```

The 1-day overlap costs almost nothing — those messages are already
in the cache and get skipped instantly via SHA-1 lookup.  But it
prevents gaps from timezone skew, delayed server indexing, or clock
differences.

### Existing `dt:` in the query

If the user's query already contains `dt:`, parse the existing range
and **update the lower bound** to `max(existing_from, last_date − 1
day)`.  The upper bound (if present) stays as-is, since the user
explicitly scoped it.  For example:

```
User query:  s:linux-kernel dt:20240101..1.month.ago
last_date:   2025-03-15

→ becomes:   s:linux-kernel dt:20250314..1.month.ago
              (lower bound raised; upper bound preserved)
```

Parsing Xapian `dt:` syntax from the query string requires finding
the `dt:` token, splitting on `..`, and interpreting relative date
expressions (e.g. `1.month.ago`).  This is a known complication; see
the **Open questions** section below.

### Query changes

When the query string changes, we do **not** rebuild the cache.  The
cache entries are per-maildir and remain valid.  We simply do a full
fetch (no `dt:` injection) and create a new entry in `queries`.
Switching back to a previous query is still incremental — its
`last_date` is preserved.

## Streaming mbox parser

The mbox format uses `From ` (with trailing space) at the start of a
line as the message separator.  In streaming mode, we read from a
`BufReader<Read>` (which wraps either a raw HTTP response or a
`GzDecoder`) and accumulate lines until we see the next `From `
separator (or EOF), at which point we have one complete message.

```
struct MboxParser<R: Read> {
    reader: BufReader<R>,        // 64 KB read buffer
    buffer: Vec<u8>,              // current message accumulator
    eof: bool,
}
```

```
next_message():
    loop:
        read one line from reader
        if EOF:
            if buffer is non-empty → yield accumulated message
            return None
        if line starts with "From " AND buffer is non-empty:
            yield accumulated message, clear buffer
        else:
            append line to buffer
```

### `From ` escaping (mboxrd)

RFC 4155 specifies that in mboxrd format, any line starting with
`>+From ` has one `>` prepended during writing.  On reading, we
strip one leading `>` from any line starting with `>+From `:

```
>From subject → From subject   (one > stripped)
>>From subject → >From subject  (one > stripped)
```

lore.kernel.org serves mboxrd format, so we must un-escape correctly.

### Read stall detection

Each `read()` call on the `BufReader` has the 30-second
`timeout_recv_body` applied via ureq's config.  If no bytes arrive
within that window, `read()` returns `Error::Timeout`.  The
`MboxParser` propagates this as `LoreError::ReadTimeout(30)`.

On a timeout, all messages already processed and written to disk are
preserved.  The cache is updated with whatever `last_date` we've seen
so far, and the `FetchResult` indicates `timed_out: true`.  The next
fetch picks up from `last_date` with the 1-day overlap, so no work is
lost.

## Per-message processing

For each complete message yielded by the parser:

1. **SHA-1 hash** of the raw bytes → hex filename.
2. **Cache check**: if the SHA-1 hex is already in `cache`, skip
   (already written to maildir).
3. **Write to maildir**: `maildir/new/<sha1hex>` via atomic write
   (write to `.tmp`, rename).
4. **Parse `Date:` header** from the message: update `newest_date`
   if this message is newer than the current tracker.

After all messages are processed (or on timeout/EOF):
- If `newest_date` is set: `cache.update_query(query, newest_date)`
- Write cache atomically (write to `.tmp`, rename).

## Error handling

### Read timeout (stall)

- Abort the current HTTP response.
- All messages already processed and written to maildir are preserved.
- Update cache with `last_date` from what we've seen so far.
- Return `FetchResult { timed_out: true, ... }`.
- The Lua thread reports a warning in the UI.
- The next fetch picks up from `last_date` with 1-day overlap.

### Anubis challenge

- Detected from response head (starts with `<` after gzip decompression).
- Buffer full HTML, parse challenge JSON, solve PoW, submit solution.
- Retry original request with auth cookie.
- Retry response goes through same content detection → streams normally.

### Network errors

- ureq returns `Error::Io` → mapped to `LoreError::Http`.
- No partial data is saved (the response hadn't started streaming yet).
- The cache is not updated.
- The user can retry.

## Open questions

1. **Parsing `dt:` from user queries.**  Xapian supports relative date
   expressions like `6.month.ago`, `1.day.ago`, `yesterday`.  To
   update the lower bound of an existing `dt:` range, we need to
   either (a) evaluate these to absolute dates client-side, or
   (b) only inject `dt:` when the query doesn't already contain one,
   and document that users should use absolute dates if they want
   incremental updates with custom `dt:` ranges.  Option (b) is simpler.

2. **`last_date` precision.**  The `Date:` header in email can have
   various timezone offsets.  We normalise to UTC before converting
   to `YYYY-MM-DD` for the `dt:` query.  The 1-day overlap compensates
   for any remaining imprecision.

3. **Read stall timeout value.**  30 seconds is conservative.  Over a
   slow connection to lore.kernel.org, individual messages may take a
   few seconds to arrive.  30 seconds of zero bytes is clearly a
   stall.  Users on very slow connections might need this configurable,
   but YAGNI until someone asks.