

User clicks a button on the UI, this calls a lua hook which, somehow, triggers a mail fetch program
like lorefetch or lei to refresh/update a maildir.

After this, our program needs to index the new mail, for a couple of reasons

1. We want to build an index of `[Message-ID (PK), Refs, Subject, Date, FileName]`  (with `In-Reply-To` appended to `Refs` if need be) - this makes it easy later to get all the data needed for the JWZ indexing algorithm. `Date` here is parsed from the `Date:` header.

2. We also want to build a FTS5 (full-text search) virtual table where for each mail, we (at minimum) support indexing and searching across:
    * subject
    * body
    * from
    * to
    * cc
    * `received_ts` — Unix timestamp from the `Received:` header (stored in `mail_ndx` as INTEGER, not in the FTS5 table — see date queries below).

### Query prefix → FTS5 column mapping

| Prefix(es) | FTS5 column | Notes |
|---|---|---|
| `s`, `subject` | `subject` | |
| `f`, `from` | `from` | |
| `b`, `body` | `body` | |
| `to` | `to` | |
| `cc` | `cc` | |
| `date` | *(not FTS5)* | Range query on `mail_ndx.received_ts`; see below. |

For undifferentiated search (no prefix) we use FTS5 with a ranking function that weights `subject` higher than `body`.

### `date:` range syntax

`date:` accepts a range of the form `N<unit>..`, `..N<unit>`, or `N1<unit>..N2<unit>`, where N is a non-negative integer and unit is one of:

| Unit | Meaning |
|---|---|
| `m` | minutes |
| `h` | hours |
| `d` | days |
| `w` | weeks |
| `mo` | months (30 days) |
| `y` | years (365 days) |

N is an offset from "now" (current UTC time). Examples:

| Syntax | Meaning |
|---|---|
| `date:3d..` | 3 days ago and older |
| `date:..1w` | within the last week |
| `date:2w..1w` | between 1 and 2 weeks ago |
| `date:0d..` | all time (degenerate: everything) |

The resolved timestamps become `WHERE n.received_ts >= ? AND n.received_ts <= ?` in the generated SQL, JOINed from `mail_ndx`.

## Schema

```sql
-- Metadata: threading data and file location.
-- One row per message.  Inserted incrementally as new mail arrives.
CREATE TABLE mail_ndx (
    message_id  TEXT PRIMARY KEY,
    refs        TEXT,          -- space-separated Message-IDs (In-Reply-To appended)
    subject     TEXT,
    date        TEXT,          -- from Date: header, display only
    received_ts INTEGER,       -- from Received: header, Unix epoch, for sorting & date: queries
    filename    TEXT NOT NULL  -- path relative to maildir root
);

CREATE INDEX idx_mail_ndx_received_ts ON mail_ndx(received_ts);

-- Full-text search.  Regular FTS5 table (no content=), stores its own data.
CREATE VIRTUAL TABLE mail_fts USING fts5 (
    message_id,
    date,       -- from Date: header
    "from",
    subject,
    "to",
    cc,
    body
);
```

For showing results in UI (eventually), we will want to do the following:
    * execute query, get MSG-ID's of matching messages
    * For each message, determine thread root -- so we are building a set of threads
    * Filter from our total set of threads down to the threads with matching messages
        * ideally/stretch goal - mark in the UI the messages that matched the query -- maybe the other messages are slightly dimmed/greyed out and the matches have a clear black text color, or something.


## Indexing
I imagine our processing to be like so

1. determine new mail — track filenames already indexed (strip flags after `:2,*`); the mail fetcher guarantees stable base filenames and only writes new mail to disk, so we only need to parse files with unknown base names

2. for each new item of mail:
    - insert [MsgId (pk), Refs (with In-Reply-To), Subject, Date, received_ts, FileName]  into a mail_ndx table
    - insert [MsgId (PK), Date, From, Subject, To, Cc, Body] into mail_fts virtual table

3. (Re-)do in-memory threading index
    - select * from mail_ndx  // this is the input to the JWZ algorithm
    - use JWZ code to build Vec<Thread>  -- store in-memory

    - Build a `HashMap<String, usize>` (Message-ID → index into `Vec<Thread>`)
        - now we can quickly find the parent thread of a message


**NOTE** - the database table `mail_ndx` and `mail_fts` are built incrementally. But the JWZ thread structure and <MsgId -> ThreadNdx> helper tables are built at each sync/program startup and held in memory


## Search
* Execute query, get a list of MsgId's of matching messages
* For each MsgId — look up thread index via `HashMap<String, usize>` → collect unique thread indices
* Extract those threads from `Vec<Thread>` → show **entire threads** (one or more messages matched)
* (Ideally) highlight matching entries in the UI somehow — matched messages at full contrast, non-matching thread siblings dimmed 



## SQLITE FTS notes

### Command-line exploration
```
sqlite <db>
```

```
.timer on
```

```
EXPLAIN QUERY PLAN <query>
```

```
PRAGMA compile_options;
...
ENABLE_FTS5
```

### Create a FTS5 virtual table
- cannot assign data types

```
CREATE VIRTUAL TABLE <tbl>
USING fts5 (
  <col>, <col1>, ...
);
```

One way to populate the index is manually:
```sql
INSERT INTO mail_fts (
    message_id,
    date,       -- from Date: header
    "from",
    subject,
    "to",
    cc,
    body
) VALUES (?, ?, ?, ?, ?, ?, ?);
```

(This is a single-row insert.  A bulk `INSERT ... SELECT` would need a source table with all
columns including `body`, which `mail_ndx` does not have.)

Actually I think we will insert one at a time as we process each piece of mail.

## TODO: Post-JWZ sorting needed

We should do additional sorting after the JWZ algorithm. Specifically, for each level of each thread we should sort siblings by date (oldest first).
I propose using the `Received` header (stored as `received_ts` in `mail_ndx`)

---

## Implementation TODO

- [x] Post-JWZ sibling sorting by `received_ts` (`thread.rs`: `sort_threads_by_date`)
- [x] `Message` trait: add `received_ts()`; `MailMessage`: extract from `Received:` header
- [x] `schema.rs`: rewrite — replace `messages`/`refs`/`messages_fts` with `mail_ndx` + `mail_fts` per Schema section above
- [x] `indexer.rs`: rewrite — insert into new `mail_ndx`/`mail_fts` tables; track filenames for new-mail detection instead of querying by message-id
- [x] `message.rs`: add `to_addr`, `cc_addr` extraction (needed for FTS5 `to`/`cc` columns)
- [x] `query.rs`: map known prefixes to FTS5 column filters (`from:` → `from:`, `subject:` → `subject:`, etc.); add `date:` range parser and SQL generation (JOIN + WHERE on `mail_ndx.received_ts`)
- [x] `query.rs`: fix SQL to use new table names (standalone `mail_fts`, no content-table JOIN needed for basic queries)
- [x] Build in-memory `HashMap<String, usize>` (MsgId → `Vec<Thread>` index) after threading
- [x] `tools/maildir-index`: update for new schema
- [x] Search→thread display pipeline (query → MsgIds → thread lookup → extract matching threads)
