// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2025 Jesper Devantier <jwd@defmacro.it>
// Ported from Go to Rust by the loreread project.

//! Fetch mailing-list threads from lore.kernel.org into a maildir.
//!
//! This crate is a faithful Rust port of the Go `lorefetch` program.
//! It contacts a public-inbox instance, solves the Anubis bot-protection
//! challenge if required, downloads matching threads as mbox, parses them,
//! and writes new messages into a maildir using SHA-1 hashes of the
//! Message-ID as filenames.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1 as Sha1Hasher};
use sha2::Sha256;

// ── Constants ────────────────────────────────────────────────────────

const BASE_URL: &str = "https://lore.kernel.org";
const USER_AGENT: &str = "Lorefetch/1.x (https://github.com/jwdevantier/lorefetch)";
const _FETCH_TIMEOUT_SECS: u64 = 60;
const CACHE_VERSION: i32 = 1;
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
}

// ── Cache ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct MaildirCache {
    version: i32,
    cache: HashSet<String>,
}

impl MaildirCache {
    #[allow(dead_code)]
    fn new() -> Self {
        Self {
            version: CACHE_VERSION,
            cache: HashSet::new(),
        }
    }

    /// Load from `<maildir>/.lorefetch-cache.json`, or initialise by
    /// scanning existing maildir files if the cache doesn't exist.
    fn load_or_init(maildir: &Path) -> Result<Self, LoreError> {
        let path = maildir.join(CACHE_FILENAME);
        if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            let cache: MaildirCache =
                serde_json::from_str(&data).map_err(|e| LoreError::CacheFormat(e.to_string()))?;
            if cache.version != CACHE_VERSION {
                // Version mismatch → rebuild from scratch
                return Self::init_from_maildir(maildir);
            }
            Ok(cache)
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
        })
    }

    fn exists(&self, key: &str) -> bool {
        self.cache.contains(key)
    }

    fn add(&mut self, key: &str) {
        self.cache.insert(key.to_string());
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

    /// Fetch the mbox for a Xapian query, solving Anubis challenges
    /// if needed.  Returns the raw mbox text.
    pub fn fetch_mbox(&self, query: &str, list: Option<&str>) -> Result<String, LoreError> {
        // ureq v3: agent() returns an Agent with cookie support enabled by default.
        // Timeout is configured via .config().timeout_read().build_agent().
        let agent = ureq::agent();

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
        let body = self.post_mbox(&agent, &search_url, &self.base_url, list)?;

        // Check if response is an Anubis challenge
        if body.contains("anubis_challenge") || body.contains("Making sure you&#39;re not a bot") {
            if self.verbose {
                eprintln!("[lorefetch] Detected Anubis bot protection, solving challenge...");
            }

            Ok(self.solve_anubis_and_retry(&agent, &body, &search_url, list)?)
        } else {
            Ok(body)
        }
    }

    fn post_mbox(
        &self,
        agent: &ureq::Agent,
        search_url: &str,
        base_url: &str,
        list: Option<&str>,
    ) -> Result<String, LoreError> {
        let _list = list; // used only for search_url construction

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
            .map_err(|e| LoreError::Http(format!("reading response: {}", e)))?;

        // lore.kernel.org sends mbox as application/gzip (not Content-Encoding).
        // If the content starts with gzip magic bytes, decompress it.
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
        list: Option<&str>,
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
        let body = self.post_mbox(agent, search_url, &self.base_url, list)?;

        // Validate
        if !body.contains("From ") {
            return Err(LoreError::NotMbox);
        }

        Ok(body)
    }
}

// ── Minimal URL-encoding (avoid pulling in a crate) ──────────────────

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
///   2. Loads or initialises the cache
///   3. Fetches the mbox content (solving Anubis if needed)
///   4. Parses the mbox into individual messages
///   5. Writes each new message to `new/`, skipping cached ones
///   6. Saves the updated cache
///
/// Returns the number of new messages written.
pub fn fetch_to_maildir(
    query: &str,
    list: Option<&str>,
    maildir: &Path,
    verbose: bool,
) -> Result<usize, LoreError> {
    // 1. Maildir structure
    if maildir.exists() {
        validate_maildir(maildir)?;
    } else {
        create_maildir(maildir)?;
    }

    // 2. Cache
    let mut cache = MaildirCache::load_or_init(maildir)?;

    // 3. Fetch mbox
    let client = LoreClient::new().verbose(verbose);
    let mbox_content = client.fetch_mbox(query, list)?;

    if mbox_content.trim().is_empty() {
        return Ok(0);
    }

    // 4. Parse
    let messages = parse_mbox(&mbox_content);
    if messages.is_empty() {
        return Err(LoreError::EmptyMbox);
    }

    // 5. Write new messages
    let new_dir = maildir.join("new");
    let mut num_saved = 0usize;

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

    // 6. Save cache
    cache.save(maildir)?;

    Ok(num_saved)
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
        // Go: sha1.Sum([]byte("<abc@def>")) → fmt.Sprintf("%x", hash)
        // This is the same as: hex(sha1("<abc@def>"))
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
        cache.save(maildir).unwrap();

        let loaded = MaildirCache::load_or_init(maildir).unwrap();
        assert!(loaded.exists("file1"));
        assert!(loaded.exists("file2"));
        assert!(!loaded.exists("file3"));
    }

    #[test]
    fn cache_version_mismatch_rebuilds() {
        let tmp = tempfile::tempdir().unwrap();
        let maildir = tmp.path();
        for subdir in &["cur", "new", "tmp"] {
            std::fs::create_dir_all(maildir.join(subdir)).unwrap();
        }

        // Write a cache with wrong version
        let bad_cache = MaildirCache {
            version: 999,
            cache: HashSet::new(),
        };
        let path = maildir.join(CACHE_FILENAME);
        std::fs::write(&path, serde_json::to_string_pretty(&bad_cache).unwrap()).unwrap();

        // Add a file to new/ so we can verify the rebuild picked it up
        std::fs::write(maildir.join("new/existing_file"), "").unwrap();

        let loaded = MaildirCache::load_or_init(maildir).unwrap();
        assert!(loaded.exists("existing_file"));
        assert_eq!(loaded.version, CACHE_VERSION);
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
