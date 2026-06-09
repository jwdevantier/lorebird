// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2025 Jesper Devantier <jwd@defmacro.it>
// Ported from Go to Rust by the loreread project.

//! Fetch mailing-list threads from lore.kernel.org into a maildir.
//!
//! This crate contacts a public-inbox instance, solves the Anubis
//! bot-protection challenge if required, downloads matching threads as
//! a gzipped mbox, parses messages, and writes new messages into a
//! maildir using SHA-1 hashes of the Message-ID as filenames.
//!
//! Incremental fetches use per-query `last_date` tracking: on
//! subsequent fetches for the same query, a `dt:` range is injected
//! to only pull new mail (with a 1-day overlap for safety).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1 as Sha1Hasher};
use sha2::Sha256;

// ── Constants ────────────────────────────────────────────────────────

const BASE_URL: &str = "https://lore.kernel.org";
const USER_AGENT: &str = "Lorefetch/1.x (https://github.com/jwdevantier/lorefetch)";
/// Read stall timeout: if no bytes arrive for this many seconds, abort.
const READ_TIMEOUT_SECS: u64 = 30;
const CACHE_VERSION: i32 = 2;
const CACHE_FILENAME: &str = ".lorefetch-cache.json";

#[cfg(unix)]
const MAIL_FILE_MODE: u32 = 0o644;
#[cfg(unix)]
const MAILDIR_MODE: u32 = 0o755;

// ── Error type ───────────────────────────────────────────────────────

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

// ── Result type ──────────────────────────────────────────────────────

/// Result of a fetch operation.
#[derive(Debug)]
pub struct FetchResult {
    /// Number of new messages written to the maildir.
    pub new_messages: usize,
    /// Total messages seen (new + already cached).
    pub total_messages: usize,
    /// Whether a read timeout occurred (partial data may have been saved).
    pub timed_out: bool,
}

// ── Cache ─────────────────────────────────────────────────────────────

/// Per-query state for incremental fetches.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct QueryState {
    /// Newest message Date: seen for this query, stored as
    /// YYYY-MM-DD for easy `dt:` range construction.
    last_date: String,
}

/// Maildir cache: tracks which SHA-1 filenames exist on disk and
/// per-query incremental state.
#[derive(Serialize, Deserialize)]
struct MaildirCache {
    version: i32,
    /// SHA-1 hex filenames already in the maildir (union across all queries).
    cache: HashSet<String>,
    /// Per-query incremental state, keyed by the original query string
    /// (without any injected `dt:` range).
    queries: HashMap<String, QueryState>,
}

impl MaildirCache {
    #[allow(dead_code)]
    fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            cache: HashSet::new(),
            queries: HashMap::new(),
        }
    }

    /// Load from `<maildir>/.lorefetch-cache.json`, or initialise by
    /// scanning existing maildir files if the cache doesn't exist or
    /// is an incompatible version.
    fn load_or_init(maildir: &Path) -> Result<Self, LoreError> {
        let path = maildir.join(CACHE_FILENAME);
        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            match serde_json::from_str::<MaildirCache>(&data) {
                Ok(cache) if cache.version == CACHE_VERSION => Ok(cache),
                _ => {
                    // Parse failure or wrong version: delete old cache, rebuild
                    let _ = std::fs::remove_file(&path);
                    Self::init_from_maildir(maildir)
                }
            }
        } else {
            Self::init_from_maildir(maildir)
        }
    }

    /// Scan `new/` and `cur/` to populate the cache from existing files.
    fn init_from_maildir(maildir: &Path) -> Result<Self, LoreError> {
        let mut cache = HashSet::new();

        // new/: all filenames are cache keys
        let new_dir = maildir.join("new");
        if new_dir.is_dir() {
            for entry in std::fs::read_dir(&new_dir)? {
                let entry = entry?;
                cache.insert(entry.file_name().to_string_lossy().to_string());
            }
        }

        // cur/: strip ":2,*" flags suffix (everything from last ":2" onward)
        let cur_dir = maildir.join("cur");
        if cur_dir.is_dir() {
            for entry in std::fs::read_dir(&cur_dir)? {
                let entry = entry?;
                let owned_name = entry.file_name().to_string_lossy().to_string();
                if let Some(idx) = owned_name.rfind(":2") {
                    cache.insert(owned_name[..idx].to_string());
                }
                // Files without ":2" are silently skipped
            }
        }

        Ok(Self {
            version: CACHE_VERSION,
            cache,
            queries: HashMap::new(),
        })
    }

    fn exists(&self, key: &str) -> bool {
        self.cache.contains(key)
    }

    fn add(&mut self, key: &str) {
        self.cache.insert(key.to_string());
    }

    /// Get the last_date for a given query, if we've seen it before.
    fn query_last_date(&self, query: &str) -> Option<&str> {
        self.queries.get(query).map(|qs| qs.last_date.as_str())
    }

    /// Update or create the per-query state after a successful fetch.
    fn update_query(&mut self, query: &str, last_date: &str) {
        self.queries.insert(
            query.to_string(),
            QueryState {
                last_date: last_date.to_string(),
            },
        );
    }

    /// Atomically write the cache back to disk (write to .tmp, then rename).
    /// On Windows, `rename` fails if the destination exists, so we
    /// remove the old file first (best-effort; OK if it doesn't exist).
    fn save(&self, maildir: &Path) -> Result<(), LoreError> {
        let path = maildir.join(CACHE_FILENAME);
        let tmp_path = path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| LoreError::CacheFormat(e.to_string()))?;
        std::fs::write(&tmp_path, &data)?;
        let _ = std::fs::remove_file(&path); // OK if file doesn't exist
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }
}

// ── Anubis challenge ─────────────────────────────────────────────────

struct AnubisChallenge {
    challenge: String,
    difficulty: usize,
}

/// Serde shape for the Anubis challenge JSON:
/// `{"challenge":"...","rules":{"algorithm":"sha256","difficulty":4,"report_as":4}}`
#[derive(Deserialize)]
struct AnubisChallengeJson {
    challenge: String,
    rules: AnubisRulesJson,
}

#[derive(Deserialize)]
struct AnubisRulesJson {
    #[allow(dead_code)]
    algorithm: String,
    difficulty: usize,
    #[allow(dead_code)]
    report_as: usize,
}

/// Detect an Anubis challenge page.  Returns `Some(AnubisChallenge)` if
/// the response body is a challenge page, `None` otherwise.
fn detect_anubis(body: &str) -> Option<AnubisChallenge> {
    if !body.contains("anubis_challenge") && !body.contains("Making sure you&#39;re not a bot") {
        return None;
    }

    // Extract the JSON blob from
    // <script id="anubis_challenge" type="application/json">...</script>
    let start_tag = r#"<script id="anubis_challenge" type="application/json">"#;
    let s_idx = body.find(start_tag)?;
    let content_start = s_idx + start_tag.len();
    let end_tag = "</script>";
    let e_idx = body[content_start..].find(end_tag)?;
    let json = &body[content_start..content_start + e_idx];

    let parsed: AnubisChallengeJson = serde_json::from_str(json).ok()?;

    Some(AnubisChallenge {
        challenge: parsed.challenge,
        difficulty: parsed.rules.difficulty,
    })
}

/// Brute-force SHA-256 proof-of-work.  Identical algorithm to the Go version.
fn solve_challenge(challenge: &str, difficulty: usize) -> (String, usize) {
    let prefix = "0".repeat(difficulty);
    for nonce in 0usize.. {
        let input = format!("{}{}", challenge, nonce);
        let hash = Sha256::digest(input.as_bytes());
        let hex = format!("{:x}", hash);
        if hex.starts_with(&prefix) {
            return (hex, nonce);
        }
    }
    unreachable!()
}

// ── Mbox parser ──────────────────────────────────────────────────────

/// Split mbox content into individual message texts.
///
/// The `"From "` separator lines are stripped from the output.  Each
/// message text includes its headers, a blank line, and the body.
fn parse_mbox(mbox_content: &str) -> Vec<String> {
    let lines: Vec<&str> = mbox_content.split('\n').collect();
    let mut messages = Vec::new();
    let mut current = String::new();

    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("From ") && i > 0 {
            if !current.is_empty() {
                messages.push(current.trim_end().to_string());
            }
            current.clear();
        } else if !line.starts_with("From ") {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }

    if !current.is_empty() {
        messages.push(current.trim_end().to_string());
    }

    messages
}

// ── Date parsing for incremental queries ──────────────────────────────

/// Parse common RFC 2822 Date: header formats and produce a `YYYY-MM-DD`
/// string for Xapian `dt:` range construction.
///
/// Handles:
///   "Fri, 06 Jun 2025 14:32:00 +0000"
///   "06 Jun 2025 14:32:00 +0000"
///
/// Also handles some variants without timezone or with different spacing.
fn parse_date_string(s: &str) -> Option<String> {
    const MONTHS: &[(&str, u8)] = &[
        ("Jan", 1),  ("Feb", 2),  ("Mar", 3),  ("Apr", 4),
        ("May", 5),  ("Jun", 6),  ("Jul", 7),  ("Aug", 8),
        ("Sep", 9),  ("Oct", 10), ("Nov", 11), ("Dec", 12),
    ];

    let parts: Vec<&str> = s.split_whitespace().collect();

    let mut day: Option<u32> = None;
    let mut month: Option<u8> = None;
    let mut year: Option<u32> = None;

    for part in &parts {
        // Try day (1-31, possibly with trailing comma)
        let trimmed = part.trim_end_matches(',');
        if let Ok(d) = trimmed.parse::<u32>() {
            if d >= 1 && d <= 31 {
                if day.is_none() {
                    day = Some(d);
                } else if year.is_none() && d >= 1990 {
                    year = Some(d);
                }
                continue;
            }
            // Year (4 digits)
            if d >= 1990 && d <= 2099 {
                year = Some(d);
                continue;
            }
        }

        // Try month name
        for (name, num) in MONTHS {
            if part.eq_ignore_ascii_case(name) {
                month = Some(*num);
                break;
            }
        }
    }

    match (year, month, day) {
        (Some(y), Some(m), Some(d)) => Some(format!("{:04}-{:02}-{:02}", y, m, d)),
        _ => None,
    }
}

/// Compute the incremental query for a given base query and last_date.
///
/// If we have a `last_date` for this query, we inject a `dt:` range:
/// `dt:YYYYMMDD..` with a 1-day overlap for safety.
///
/// If the query already contains `dt:`, we update the lower bound.
fn build_incremental_query(query: &str, last_date: &str) -> String {
    // last_date is "YYYY-MM-DD" → extract YYYY, MM, DD
    let ymd: Vec<&str> = last_date.split('-').collect();
    if ymd.len() != 3 {
        return query.to_string();
    }
    let year: i32 = match ymd[0].parse() {
        Ok(y) => y,
        Err(_) => return query.to_string(),
    };
    let month: i32 = match ymd[1].parse() {
        Ok(m) => m,
        Err(_) => return query.to_string(),
    };
    let day: i32 = match ymd[2].parse() {
        Ok(d) => d,
        Err(_) => return query.to_string(),
    };
    if year == 0 || month == 0 || day == 0 {
        return query.to_string();
    }

    // Subtract 1 day for overlap
    let (y, m, d) = subtract_one_day(year, month, day);
    let dt_lower = format!("{:04}{:02}{:02}", y, m, d);

    // Check if the query already has dt:
    if let Some(dt_start) = query.find("dt:") {
        // Find the end of the dt: clause (next space or end of string)
        let after_dt = &query[dt_start..];
        let dt_end = after_dt
            .find(' ')
            .map(|i| dt_start + i)
            .unwrap_or(query.len());

        let dt_clause = &query[dt_start..dt_end];
        if let Some(dotdot) = dt_clause.find("..") {
            // dt:XXX..YYY or dt:XXX.. → replace lower bound
            let new_dt = format!("dt:{}{}", dt_lower, &dt_clause[dotdot..]);
            format!("{}{}{}", &query[..dt_start], new_dt, &query[dt_end..])
        } else {
            // dt:XXX without .. — unusual, convert to range
            let new_dt = format!("dt:{}..", dt_lower);
            format!("{}{}{}", &query[..dt_start], new_dt, &query[dt_end..])
        }
    } else {
        // No dt: in query, append one
        format!("{} dt:{}..", query.trim_end(), dt_lower)
    }
}

/// Subtract one day from a (year, month, day) tuple.
fn subtract_one_day(year: i32, month: i32, day: i32) -> (i32, i32, i32) {
    const DAYS_IN_MONTH: [i32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    if day > 1 {
        (year, month, day - 1)
    } else if month > 1 {
        let prev_month = month - 1;
        let days_in_prev = if prev_month == 2 && is_leap_year(year) {
            29
        } else {
            DAYS_IN_MONTH[(prev_month - 1) as usize]
        };
        (year, prev_month, days_in_prev)
    } else {
        // January 1 → December 31 of previous year
        (year - 1, 12, 31)
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

// ── Maildir helpers ──────────────────────────────────────────────────

/// Check that `path` contains `cur/`, `new/`, `tmp/`.
fn validate_maildir(path: &Path) -> Result<(), LoreError> {
    for subdir in &["cur", "new", "tmp"] {
        let p = path.join(subdir);
        if !p.is_dir() {
            return Err(LoreError::Maildir(format!(
                "invalid maildir '{}' — sub-directory '{}' missing",
                path.display(),
                subdir
            )));
        }
    }
    Ok(())
}

/// Create `cur/`, `new/`, `tmp/` under `path`.
fn create_maildir(path: &Path) -> Result<(), LoreError> {
    for subdir in &["cur", "new", "tmp"] {
        let p = path.join(subdir);
        std::fs::create_dir_all(&p).map_err(|e| {
            LoreError::Maildir(format!("creating directory {}: {}", p.display(), e))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(MAILDIR_MODE))?;
        }
    }
    Ok(())
}

// ── HTTP client ──────────────────────────────────────────────────────

/// HTTP client for lore.kernel.org (or other public-inbox instances).
///
/// Handles Anubis bot-protection challenges automatically.
pub struct LoreClient {
    base_url: String,
    verbose: bool,
}

impl LoreClient {
    /// Create a client targeting `https://lore.kernel.org`.
    pub fn new() -> Self {
        Self {
            base_url: BASE_URL.to_string(),
            verbose: false,
        }
    }

    /// Create a client targeting a different public-inbox instance.
    pub fn with_base_url(url: &str) -> Self {
        Self {
            base_url: url.to_string(),
            verbose: false,
        }
    }

    /// Enable verbose logging to stderr.
    pub fn verbose(mut self, v: bool) -> Self {
        self.verbose = v;
        self
    }

    /// Build a ureq agent with a read-stall timeout.
    /// `timeout_recv_body` triggers if no data arrives for the given duration.
    fn build_agent(&self) -> ureq::Agent {
        use ureq::config::Config;
        Config::builder()
            .timeout_recv_body(Some(Duration::from_secs(READ_TIMEOUT_SECS)))
            .build()
            .into()
    }

    /// Fetch the mbox for a Xapian query, solving Anubis challenges if needed.
    /// Returns the raw mbox text (decompressed if gzipped).
    pub fn fetch_mbox(&self, query: &str, list: Option<&str>) -> Result<String, LoreError> {
        let agent = self.build_agent();

        // Build search URL
        let base_search = match list {
            Some(l) => format!("{}/{}/", self.base_url, l),
            None => format!("{}/all/", self.base_url),
        };

        // URL with query parameters
        let search_url = format!("{}?q={}&x=m", base_search, urlencoding(query));

        if self.verbose {
            eprintln!("[lorefetch] Fetching mbox from: {}", search_url);
            eprintln!("[lorefetch] Query: {}", query);
        }

        // Try the initial POST request
        let body = self.post_mbox(&agent, &search_url, &self.base_url)?;

        // Check if response is an Anubis challenge
        if body.contains("anubis_challenge") || body.contains("Making sure you&#39;re not a bot") {
            if self.verbose {
                eprintln!("[lorefetch] Detected Anubis bot protection, solving challenge...");
            }
            self.solve_anubis_and_retry(&agent, &body, &search_url)
        } else {
            Ok(body)
        }
    }

    fn post_mbox(
        &self,
        agent: &ureq::Agent,
        search_url: &str,
        base_url: &str,
    ) -> Result<String, LoreError> {
        let response = agent
            .post(search_url)
            .header("User-Agent", USER_AGENT)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("Accept-Language", "en-US,en;q=0.5")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Origin", base_url)
            .header("Connection", "keep-alive")
            .header("Referer", search_url)
            .header("Upgrade-Insecure-Requests", "1")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-User", "?1")
            .header("Priority", "u=0, i")
            .send_form([("x", "full threads")])
            .map_err(|e| LoreError::Http(format!("POST request failed: {}", e)))?;

        let status = response.status();
        if status != 200 {
            return Err(LoreError::Http(format!(
                "request returned status {}",
                status
            )));
        }

        // Read body with generous limit and lossy UTF-8 (mbox data may
        // contain non-UTF-8 bytes from various mail encodings).
        // ureq's default body limit is 10MB; lore.kernel.org results
        // can be much larger.  We read as bytes first, then convert
        // lossily — this is more robust than read_to_string().
        let bytes = response
            .into_body()
            .with_config()
            .limit(300 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| {
                // ureq v3: timeouts come as Error::Timeout, IO errors as Error::Io
                match &e {
                    ureq::Error::Timeout(_) => LoreError::ReadTimeout(READ_TIMEOUT_SECS),
                    _ => LoreError::Http(format!("reading response: {}", e)),
                }
            })?;

        // gzip detection: lore.kernel.org sends Content-Type: application/gzip
        let body = if bytes.len() > 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
            if self.verbose {
                eprintln!("[lorefetch] Detected gzipped content, decompressing...");
            }
            let mut gz = flate2::read::GzDecoder::new(&bytes[..]);
            let mut decompressed = Vec::new();
            gz.read_to_end(&mut decompressed)
                .map_err(|e| LoreError::Http(format!("decompressing gzip: {}", e)))?;
            String::from_utf8_lossy(&decompressed).to_string()
        } else {
            String::from_utf8_lossy(&bytes).to_string()
        };

        if self.verbose {
            eprintln!(
                "[lorefetch] Received {} bytes ({} lines)",
                body.len(),
                body.lines().count()
            );
        }

        Ok(body)
    }

    fn solve_anubis_and_retry(
        &self,
        agent: &ureq::Agent,
        body: &str,
        search_url: &str,
    ) -> Result<String, LoreError> {
        let challenge = detect_anubis(body).ok_or_else(|| {
            LoreError::Anubis("could not extract Anubis challenge data".to_string())
        })?;

        if self.verbose {
            eprintln!(
                "[lorefetch] Solving Anubis challenge: {} (difficulty {})",
                challenge.challenge, challenge.difficulty
            );
        }

        let (hash_hex, nonce) = solve_challenge(&challenge.challenge, challenge.difficulty);

        if self.verbose {
            eprintln!("[lorefetch] Found solution: nonce={}", nonce);
        }

        // Submit the solution
        let pass_url = format!(
            "{}/.within.website/x/cmd/anubis/api/pass-challenge?response={}&nonce={}&redir={}&elapsedTime=5000",
            self.base_url,
            urlencoding(&hash_hex),
            nonce,
            urlencoding(search_url),
        );

        if self.verbose {
            eprintln!("[lorefetch] Submitting challenge solution");
        }

        let _ = agent
            .get(&pass_url)
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .header("User-Agent", USER_AGENT)
            .header("Accept-Language", "en-US,en;q=0.5")
            .header("Referer", search_url)
            .header("Upgrade-Insecure-Requests", "1")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "same-origin")
            .call()
            .map_err(|e| LoreError::Anubis(format!("submitting challenge solution: {}", e)))?;

        if self.verbose {
            eprintln!("[lorefetch] Challenge submitted, retrying original request...");
        }

        // Retry the original POST — the cookie jar now has the auth cookie
        let body = self.post_mbox(agent, search_url, &self.base_url)?;

        // Validate
        if !body.contains("From ") {
            return Err(LoreError::NotMbox);
        }

        Ok(body)
    }
}

// ── Minimal URL-encoding ─────────────────────────────────────────────

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", byte));
            }
        }
    }
    out
}

// ── Public API: fetch to maildir ─────────────────────────────────────

/// Fetch mail from lore.kernel.org and write new messages into a maildir.
///
/// This is the primary entry point.  It:
///   1. Creates or validates the maildir structure
///   2. Loads or initialises the cache (including per-query `last_date`)
///   3. Computes the incremental query (injecting `dt:` if we have a
///      `last_date` for this query)
///   4. Fetches the mbox content (solving Anubis if needed)
///   5. Parses the mbox into individual messages
///   6. Writes each new message to `new/`, skipping cached ones
///   7. Tracks the newest Date: header for incremental state
///   8. Saves the updated cache atomically
///
/// Returns a `FetchResult` with counts and timeout status.
pub fn fetch_to_maildir(
    query: &str,
    list: Option<&str>,
    maildir: &Path,
    verbose: bool,
) -> Result<FetchResult, LoreError> {
    // 1. Maildir structure
    if maildir.exists() {
        validate_maildir(maildir)?;
    } else {
        create_maildir(maildir)?;
    }

    // 2. Cache
    let mut cache = MaildirCache::load_or_init(maildir)?;

    // 3. Compute incremental query
    let effective_query = match cache.query_last_date(query) {
        Some(last_date) => {
            if verbose {
                eprintln!(
                    "[lorefetch] Incremental fetch: last_date={}, injecting dt: range",
                    last_date
                );
            }
            build_incremental_query(query, last_date)
        }
        None => {
            if verbose {
                eprintln!("[lorefetch] Full fetch (no last_date for this query)");
            }
            query.to_string()
        }
    };

    // 4. Fetch mbox
    let client = LoreClient::new().verbose(verbose);
    let mbox_content = client.fetch_mbox(&effective_query, list)?;

    if mbox_content.trim().is_empty() {
        return Ok(FetchResult {
            new_messages: 0,
            total_messages: 0,
            timed_out: false,
        });
    }

    // 5. Parse
    let messages = parse_mbox(&mbox_content);
    if messages.is_empty() {
        return Err(LoreError::EmptyMbox);
    }

    // 6. Process each message
    let new_dir = maildir.join("new");
    let mut num_saved = 0usize;
    let mut newest_date: Option<String> = None;

    for (i, msg_text) in messages.iter().enumerate() {
        let parsed = mail_parser::MessageParser::default()
            .parse(msg_text.as_bytes())
            .ok_or_else(|| LoreError::MissingMessageId { index: i })?;

        let msg_id = parsed
            .message_id()
            .ok_or_else(|| LoreError::MissingMessageId { index: i })?
            .to_string();

        // SHA-1 hash of Message-ID (including angle brackets, matching Go)
        let mut hasher = Sha1Hasher::new();
        hasher.update(msg_id.as_bytes());
        let hash = hasher.finalize();
        let filename = format!("{:x}", hash);

        // Track newest Date: for incremental queries
        if let Some(date_hdr) = parsed.date() {
            if let Some(xapian_date) = parse_date_string(&date_hdr.to_rfc822()) {
                match &newest_date {
                    None => newest_date = Some(xapian_date),
                    Some(current) => {
                        if &xapian_date > current {
                            newest_date = Some(xapian_date);
                        }
                    }
                }
            }
        }

        if cache.exists(&filename) {
            continue;
        }

        let file_path = new_dir.join(&filename);
        std::fs::write(&file_path, msg_text.as_bytes())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(MAIL_FILE_MODE))?;
        }

        cache.add(&filename);
        num_saved += 1;
    }

    // 7. Update per-query state and save cache
    if let Some(ref last_date) = newest_date {
        cache.update_query(query, last_date);
    }
    cache.save(maildir)?;

    let total = messages.len();
    Ok(FetchResult {
        new_messages: num_saved,
        total_messages: total,
        timed_out: false,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Mbox parser tests ──────────────────────────────────────────

    #[test]
    fn parse_empty_mbox() {
        let messages = parse_mbox("");
        assert!(messages.is_empty());
    }

    #[test]
    fn parse_single_message() {
        let mbox = "From sender@example.com Mon Jan  2 03:14:15 2024\n\
                     Message-ID: <abc@def>\n\
                     Subject: Hello\n\
                     \n\
                     Body here\n";
        let messages = parse_mbox(mbox);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("Message-ID: <abc@def>"));
        assert!(messages[0].contains("Body here"));
        // The "From " separator line should NOT be in the message text
        assert!(!messages[0].starts_with("From "));
    }

    #[test]
    fn parse_multiple_messages() {
        let mbox = "From a@b.com Mon Jan  1 00:00:00 2024\n\
                     Message-ID: <first@test>\n\
                     \n\
                     First\n\
                     \n\
                     From b@c.com Mon Jan  2 00:00:00 2024\n\
                     Message-ID: <second@test>\n\
                     \n\
                     Second\n";
        let messages = parse_mbox(mbox);
        assert_eq!(messages.len(), 2);
        assert!(messages[0].contains("first@test"));
        assert!(messages[1].contains("second@test"));
    }

    // ── Anubis challenge solver ─────────────────────────────────────

    #[test]
    fn solve_trivial_challenge() {
        // difficulty 0 → any hash works, nonce 0 should be the answer
        let (hash, nonce) = solve_challenge("test", 0);
        assert_eq!(nonce, 0);
        assert!(!hash.is_empty());
    }

    #[test]
    fn solve_difficulty_1() {
        let (hash, nonce) = solve_challenge("test", 1);
        assert!(hash.starts_with('0'));
        assert!(
            format!(
                "{:x}",
                sha2::Sha256::digest(format!("test{}", nonce).as_bytes())
            )
            .starts_with('0')
        );
    }

    // ── Anubis detection ───────────────────────────────────────────

    #[test]
    fn detect_anubis_challenge_present() {
        let html = r#"<html><script id="anubis_challenge" type="application/json">{"challenge":"abc123","rules":{"algorithm":"sha256","difficulty":4,"report_as":4}}</script></html>"#;
        let result = detect_anubis(html);
        assert!(result.is_some());
        let ch = result.unwrap();
        assert_eq!(ch.challenge, "abc123");
        assert_eq!(ch.difficulty, 4);
    }

    #[test]
    fn detect_anubis_not_present() {
        let html = "<html><body>Normal page</body></html>";
        assert!(detect_anubis(html).is_none());
    }

    #[test]
    fn detect_anubis_with_html_entity() {
        let html = r#"<html><p>Making sure you&#39;re not a bot</p></html>"#;
        assert!(detect_anubis(html).is_none()); // No challenge data, just the entity
    }

    // ── Anubis JSON parsing (serde) ─────────────────────────────────

    #[test]
    fn anubis_json_structured_parsing() {
        // Uses real JSON with nested rules object — serde parses it properly
        let html = r#"<html><script id="anubis_challenge" type="application/json">{"challenge":"abc123def","rules":{"algorithm":"sha256","difficulty":5,"report_as":5}}</script></html>"#;
        let result = detect_anubis(html);
        assert!(result.is_some());
        let ch = result.unwrap();
        assert_eq!(ch.challenge, "abc123def");
        assert_eq!(ch.difficulty, 5);
    }

    // ── URL encoding ───────────────────────────────────────────────

    #[test]
    fn urlencoding_basic() {
        assert_eq!(urlencoding("hello"), "hello");
        assert_eq!(urlencoding("a b"), "a%20b");
        assert_eq!(urlencoding("a=b"), "a%3Db");
    }

    // ── SHA-1 naming ──────────────────────────────────────────────

    #[test]
    fn sha1_filename_matches_go() {
        let msg_id = "<abc@def>";
        let mut hasher = Sha1Hasher::new();
        hasher.update(msg_id.as_bytes());
        let hash = hasher.finalize();
        let filename = format!("{:x}", hash);
        // Verify it's 40 hex chars (SHA-1 output)
        assert_eq!(filename.len(), 40);
        // Verify it's all lowercase hex
        assert!(filename.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── Date parsing ──────────────────────────────────────────────

    #[test]
    fn parse_rfc2822_date() {
        assert_eq!(
            parse_date_string("Fri, 06 Jun 2025 14:32:00 +0000"),
            Some("2025-06-06".to_string())
        );
    }

    #[test]
    fn parse_rfc2822_date_no_day_name() {
        assert_eq!(
            parse_date_string("06 Jun 2025 14:32:00 +0000"),
            Some("2025-06-06".to_string())
        );
    }

    #[test]
    fn parse_different_month() {
        assert_eq!(
            parse_date_string("15 Mar 2024 09:00:00 -0500"),
            Some("2024-03-15".to_string())
        );
    }

    #[test]
    fn parse_date_fails_on_garbage() {
        assert_eq!(parse_date_string("not a date"), None);
    }

    // ── Incremental queries ────────────────────────────────────────

    #[test]
    fn incremental_query_no_last_date() {
        let query = "s:linux-kernel";
        let cache = MaildirCache::new();
        assert_eq!(cache.query_last_date(query), None);
    }

    #[test]
    fn incremental_query_with_last_date() {
        let result = build_incremental_query("s:linux-kernel", "2025-06-08");
        assert_eq!(result, "s:linux-kernel dt:20250607..");
    }

    #[test]
    fn incremental_query_with_existing_dt() {
        let result = build_incremental_query(
            "s:linux-kernel dt:20240101..1.month.ago",
            "2025-03-15",
        );
        assert_eq!(result, "s:linux-kernel dt:20250314..1.month.ago");
    }

    #[test]
    fn incremental_query_existing_dt_no_range() {
        let result = build_incremental_query(
            "s:linux-kernel dt:20240101",
            "2025-03-15",
        );
        assert!(result.contains("dt:20250314.."));
    }

    // ── Day subtraction ────────────────────────────────────────────

    #[test]
    fn subtract_one_day_simple() {
        assert_eq!(subtract_one_day(2025, 6, 8), (2025, 6, 7));
    }

    #[test]
    fn subtract_one_day_month_boundary() {
        assert_eq!(subtract_one_day(2025, 6, 1), (2025, 5, 31));
    }

    #[test]
    fn subtract_one_day_year_boundary() {
        assert_eq!(subtract_one_day(2025, 1, 1), (2024, 12, 31));
    }

    #[test]
    fn subtract_one_day_leap_year() {
        // March 1, 2024 → Feb 29, 2024 (2024 is a leap year)
        assert_eq!(subtract_one_day(2024, 3, 1), (2024, 2, 29));
    }

    // ── Maildir cache ──────────────────────────────────────────────

    #[test]
    fn cache_init_from_maildir() {
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path();

        // Create maildir structure
        for subdir in &["cur", "new", "tmp"] {
            std::fs::create_dir_all(maildir.join(subdir)).unwrap();
        }

        // Add files to new/
        std::fs::write(maildir.join("new/abc123"), "").unwrap();
        std::fs::write(maildir.join("new/def456"), "").unwrap();

        // Add files to cur/ with flags
        std::fs::write(maildir.join("cur/ghi789:2,S"), "").unwrap();
        std::fs::write(maildir.join("cur/jkl012:2,RS"), "").unwrap();

        let cache = MaildirCache::init_from_maildir(maildir).unwrap();
        assert!(cache.exists("abc123"));
        assert!(cache.exists("def456"));
        // cur/ files should be indexed by base name without flags
        assert!(cache.exists("ghi789"));
        assert!(cache.exists("jkl012"));
        // The full cur/ filenames should NOT be in the cache
        assert!(!cache.exists("ghi789:2,S"));
    }

    #[test]
    fn cache_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path();
        for subdir in &["cur", "new", "tmp"] {
            std::fs::create_dir_all(maildir.join(subdir)).unwrap();
        }

        let mut cache = MaildirCache::new();
        cache.add("file1");
        cache.add("file2");
        cache.update_query("s:linux-kernel", "2025-06-08");
        cache.save(maildir).unwrap();

        let loaded = MaildirCache::load_or_init(maildir).unwrap();
        assert!(loaded.exists("file1"));
        assert!(loaded.exists("file2"));
        assert!(!loaded.exists("file3"));
        assert_eq!(loaded.query_last_date("s:linux-kernel"), Some("2025-06-08"));
    }

    #[test]
    fn cache_version_mismatch_deletes_and_rebuilds() {
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path();
        for subdir in &["cur", "new", "tmp"] {
            std::fs::create_dir_all(maildir.join(subdir)).unwrap();
        }

        // Write a cache with wrong version
        let bad_cache = MaildirCache {
            version: 999,
            cache: HashSet::new(),
            queries: HashMap::new(),
        };
        let path = maildir.join(CACHE_FILENAME);
        std::fs::write(&path, serde_json::to_string_pretty(&bad_cache).unwrap()).unwrap();

        // Add a file to new/ so we can verify the rebuild picked it up
        std::fs::write(maildir.join("new/existing_file"), "").unwrap();

        let loaded = MaildirCache::load_or_init(maildir).unwrap();
        assert!(loaded.exists("existing_file"));
        assert_eq!(loaded.version, CACHE_VERSION);
        // Old invalid cache file should have been deleted
        assert!(!path.exists());
    }

    #[test]
    fn cache_per_query_state() {
        let mut cache = MaildirCache::new();
        assert_eq!(cache.query_last_date("s:linux-kernel"), None);

        cache.update_query("s:linux-kernel", "2025-06-08");
        assert_eq!(
            cache.query_last_date("s:linux-kernel"),
            Some("2025-06-08")
        );

        assert_eq!(cache.query_last_date("s:linux-raid"), None);

        cache.update_query("s:linux-raid", "2025-05-01");
        assert_eq!(cache.query_last_date("s:linux-raid"), Some("2025-05-01"));

        // Original query still has its state
        assert_eq!(
            cache.query_last_date("s:linux-kernel"),
            Some("2025-06-08")
        );
    }

    #[test]
    fn cache_corrupt_or_v1_deletes_and_rebuilds() {
        // A corrupt or incompatible cache file is deleted and rebuilt
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path();
        for subdir in &["cur", "new", "tmp"] {
            std::fs::create_dir_all(maildir.join(subdir)).unwrap();
        }

        // v1 cache (no queries field) — can't deserialize into v2 struct
        let v1_cache = r#"{"version":1,"cache":["abc123"]}"#;
        let path = maildir.join(CACHE_FILENAME);
        std::fs::write(&path, v1_cache).unwrap();
        // Put the file in new/ so rebuild picks it up
        std::fs::write(maildir.join("new/abc123"), "").unwrap();

        let loaded = MaildirCache::load_or_init(maildir).unwrap();
        assert_eq!(loaded.version, CACHE_VERSION);
        assert!(loaded.exists("abc123"));
        assert!(loaded.queries.is_empty());
        // Old incompatible cache file should have been deleted
        assert!(!path.exists());
    }

    // ── Maildir validation ────────────────────────────────────────

    #[test]
    fn validate_maildir_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path();
        for subdir in &["cur", "new", "tmp"] {
            std::fs::create_dir_all(maildir.join(subdir)).unwrap();
        }
        assert!(validate_maildir(maildir).is_ok());
    }

    #[test]
    fn validate_maildir_missing_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path();
        std::fs::create_dir_all(maildir.join("cur")).unwrap();
        // Missing new/ and tmp/
        assert!(validate_maildir(maildir).is_err());
    }

    #[test]
    fn create_maildir_from_scratch() {
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path().join("my-mail");
        create_maildir(&maildir).unwrap();
        assert!(maildir.join("cur").is_dir());
        assert!(maildir.join("new").is_dir());
        assert!(maildir.join("tmp").is_dir());
    }
}