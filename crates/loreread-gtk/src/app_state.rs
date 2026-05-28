//! Application state — database connection, config, thread data.
//!
//! `AppState` is the shared mutable state that the GUI reads and writes.
//! It holds the SQLite connection, the loaded config, and the root
//! list-store of `ThreadNode`s that backs the `ColumnView`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use gio::ListStore;
use rusqlite::Connection;

use crate::thread_node::ThreadNode;
use loreread_core::store::DbMessage;
use loreread_core::thread::{self, Thread};
use loreread_lua::{AppConfig, LoadedConfig, ResolvedProfile, Vm};

/// Central application state, shared between the window and action callbacks.
pub struct AppState {
    /// SQLite connection to the mail index database.
    pub db: RefCell<Option<Connection>>,

    /// Path to the DB's associated maildir (empty = no DB open).
    pub db_maildir: RefCell<PathBuf>,

    /// Root list-store backing the `ColumnView` tree.
    pub root_model: ListStore,

    /// The profile label currently active in the sidebar.
    pub active_profile: RefCell<String>,

    /// Path to the maildir of the currently active profile.
    pub active_maildir: RefCell<PathBuf>,

    /// Optional active view query (when a view is selected rather
    /// than "All Mail").
    pub active_query: RefCell<Option<String>>,

    /// Lua VM — kept alive so that hook handles remain valid.
    pub vm: Vm,

    /// Loaded configuration (resolved profiles + hooks).
    pub config: LoadedConfig,

    /// Resolved profiles (keyed by label), derived from config.
    pub profiles: HashMap<String, ResolvedProfile>,
}

impl AppState {
    /// Create a new `AppState`, loading config from `config_path`
    /// (or a default location).  Falls back to an empty config if
    /// the file cannot be loaded.
    pub fn new(config_path: Option<&std::path::Path>) -> Self {
        let vm = Vm::new().expect("failed to create Lua VM");
        let (config, profiles) = match config_path {
            Some(path) => match vm.load_config_file(path) {
                Ok(loaded) => {
                    let resolved = loaded.config.resolve_all();
                    eprintln!(
                        "[loreread] loaded config: {} profile(s) from {}",
                        resolved.len(),
                        path.display()
                    );
                    (loaded, resolved)
                }
                Err(e) => {
                    eprintln!(
                        "[loreread] warning: failed to load config from {}: {}",
                        path.display(),
                        e
                    );
                    let empty_config = AppConfig {
                        user: None,
                        profiles: HashMap::new(),
                    };
                    let loaded = LoadedConfig {
                        config: empty_config.clone(),
                        profile_hooks: HashMap::new(),
                        global_hooks: loreread_lua::GlobalHooks {
                            on_reply: None,
                            on_send: None,
                        },
                    };
                    (loaded, empty_config.resolve_all())
                }
            },
            None => {
                // Try default location: ~/.config/loreread/config.lua
                let default = dirs_for_loreread();
                let cfg_file = default.join("config.lua");
                if cfg_file.exists() {
                    match vm.load_config_file(&cfg_file) {
                        Ok(loaded) => {
                            let resolved = loaded.config.resolve_all();
                            eprintln!(
                                "[loreread] loaded config: {} profile(s) from {}",
                                resolved.len(),
                                cfg_file.display()
                            );
                            (loaded, resolved)
                        }
                        Err(e) => {
                            eprintln!(
                                "[loreread] warning: failed to load config from {}: {}",
                                cfg_file.display(),
                                e
                            );
                            let empty = empty_config();
                            let resolved = empty.config.resolve_all();
                            (empty, resolved)
                        }
                    }
                } else {
                    eprintln!(
                        "[loreread] no config found at {} — using empty config",
                        cfg_file.display()
                    );
                    let empty = empty_config();
                    let resolved = empty.config.resolve_all();
                    (empty, resolved)
                }
            }
        };

        Self {
            db: RefCell::new(None),
            db_maildir: RefCell::new(PathBuf::new()),
            root_model: ListStore::new::<ThreadNode>(),
            active_profile: RefCell::new(String::new()),
            active_maildir: RefCell::new(PathBuf::new()),
            active_query: RefCell::new(None),
            vm,
            config,
            profiles,
        }
    }

    /// Select a profile by label: sets `active_profile` and `active_maildir`.
    /// If the maildir differs from the currently-open DB, the DB will be
    /// re-opened on next index.
    pub fn select_profile(&self, label: &str) {
        if let Some(profile) = self.profiles.get(label) {
            *self.active_profile.borrow_mut() = label.to_string();
            *self.active_maildir.borrow_mut() = profile.maildir.clone();
            *self.active_query.borrow_mut() = None;

            // If DB is open for a different maildir, close it
            // so the next index_and_rebuild() will re-open.
            let current_db_dir = self.db_maildir.borrow().clone();
            if current_db_dir != profile.maildir {
                *self.db.borrow_mut() = None;
            }
        }
    }

    /// Select a view within the current profile.
    /// Sets `active_query` to the view's query string.
    pub fn select_view(&self, query: String) {
        *self.active_query.borrow_mut() = Some(query);
    }

    /// Clear the view filter (show all mail for the active profile).
    pub fn clear_view(&self) {
        *self.active_query.borrow_mut() = None;
    }

    /// Open (or create) the index database for `maildir`.
    ///
    /// The database is stored at `{maildir}/.loreread.db`.
    /// Creates the schema if the file does not yet exist.
    pub fn open_db(&self, maildir: &std::path::Path) -> Result<(), String> {
        let db_path = maildir.join(".loreread.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("cannot open database: {}", e))?;
        loreread_core::schema::init_db(&conn)
            .map_err(|e| format!("cannot init schema: {}", e))?;
        *self.db.borrow_mut() = Some(conn);
        *self.db_maildir.borrow_mut() = maildir.to_path_buf();
        Ok(())
    }

    /// Index the active maildir and rebuild the thread tree.
    ///
    /// Returns the number of newly indexed messages, or an error string.
    pub fn index_and_rebuild(&self) -> Result<usize, String> {
        let maildir = self.active_maildir.borrow().clone();
        if maildir.as_os_str().is_empty() {
            return Err("no profile selected — pick a profile from the sidebar".to_string());
        }

        // Open DB if needed
        let need_open = self.db.borrow().is_none()
            || self.db_maildir.borrow().as_path() != maildir.as_path();
        if need_open {
            self.open_db(&maildir)?;
        }

        // Index (needs exclusive borrow of conn)
        let inserted = {
            let db = self.db.borrow();
            let conn = db.as_ref().ok_or("database not open")?;
            loreread_core::indexer::index_maildir(conn, &maildir)
                .map_err(|e| format!("indexing failed: {}", e))?
        };

        // Rebuild thread tree (separate borrow)
        {
            let db = self.db.borrow();
            let conn = db.as_ref().ok_or("database not open")?;

            // If a view query is active, filter messages
            let query_str = self.active_query.borrow();
            if let Some(ref q) = *query_str {
                self.rebuild_thread_tree_filtered(conn, &maildir, q)?;
            } else {
                self.rebuild_thread_tree(conn, &maildir)?;
            }
        }

        Ok(inserted)
    }

    /// Rebuild the thread tree from the database (all messages).
    pub fn rebuild_thread_tree(
        &self,
        conn: &Connection,
        maildir: &std::path::Path,
    ) -> Result<(), String> {
        let messages = loreread_core::store::load_all_messages(conn)
            .map_err(|e| format!("query failed: {}", e))?;

        let threads = thread::thread_messages(messages);

        self.root_model.remove_all();
        let node_tree = ThreadNodeTree::from_threads(&threads, maildir);
        for root_node in &node_tree.roots {
            self.root_model.append(root_node);
        }

        Ok(())
    }

    /// Rebuild the thread tree, filtering to only those threads that
    /// contain at least one message matching the query.
    pub fn rebuild_thread_tree_filtered(
        &self,
        conn: &Connection,
        maildir: &std::path::Path,
        query: &str,
    ) -> Result<(), String> {
        let parsed = loreread_core::query::parse_query(query)
            .map_err(|e| format!("bad query '{}': {:?}", query, e))?;
        let pq = loreread_core::query::ParsedQuery::from_ast(&parsed, 500);

        let matching_ids = loreread_core::query::search(conn, &pq)
            .map_err(|e| format!("search failed: {}", e))?;

        // Load matching messages and thread them
        let messages = loreread_core::store::load_messages_by_ids(conn, &matching_ids)
            .map_err(|e| format!("query failed: {}", e))?;

        let threads = thread::thread_messages(messages);

        self.root_model.remove_all();
        let node_tree = ThreadNodeTree::from_threads(&threads, maildir);
        for root_node in &node_tree.roots {
            self.root_model.append(root_node);
        }

        Ok(())
    }

    /// Call the on_fetch Lua hook for the active profile.
    /// Returns `Ok(true)` if the hook succeeded and wants re-indexing,
    /// `Ok(false)` if it returned falsy, `Err` on failure.
    pub fn call_fetch_hook(&self) -> Result<bool, String> {
        let label = self.active_profile.borrow().clone();
        if label.is_empty() {
            return Err("no profile selected".to_string());
        }
        let hooks = self.config.profile_hooks.get(&label)
            .ok_or_else(|| format!("no hooks for profile '{}'", label))?;
        self.vm.call_on_fetch(&label, hooks)
            .map_err(|e| format!("on_fetch hook failed: {}", e))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn empty_config() -> LoadedConfig {
    LoadedConfig {
        config: AppConfig {
            user: None,
            profiles: HashMap::new(),
        },
        profile_hooks: HashMap::new(),
        global_hooks: loreread_lua::GlobalHooks {
            on_reply: None,
            on_send: None,
        },
    }
}

/// Return the XDG config directory for loreread.
fn dirs_for_loreread() -> PathBuf {
    // Use xdg config dir, falling back to ~/.config
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs_sys_config()
        })
        .join("loreread")
}

fn dirs_sys_config() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config")
}

/// Intermediate tree of `ThreadNode` GObjects built from
/// `Thread<DbMessage>`.
struct ThreadNodeTree {
    roots: Vec<ThreadNode>,
}

impl ThreadNodeTree {
    /// Recursively convert `Thread<DbMessage>` objects into
    /// `ThreadNode` GObjects with display-friendly date strings.
    fn from_threads(threads: &[Thread<DbMessage>], maildir: &std::path::Path) -> Self {
        let roots: Vec<ThreadNode> = threads
            .iter()
            .map(|t| Self::build_node(t, maildir))
            .collect();
        Self { roots }
    }

    fn build_node(t: &Thread<DbMessage>, maildir: &std::path::Path) -> ThreadNode {
        let msg = t.message.as_ref();

        let (from, body) = msg
            .as_ref()
            .and_then(|m| {
                loreread_core::store::read_raw_message(maildir, &m.filename)
                    .map(|parsed| {
                        (
                            parsed.from_addr.unwrap_or_else(|| m.from_addr.clone().unwrap_or_default()),
                            parsed.body_text.unwrap_or_default(),
                        )
                    })
            })
            .unwrap_or_else(|| {
                (
                    msg.as_ref()
                        .and_then(|m| m.from_addr.clone())
                        .unwrap_or_default(),
                    String::new(),
                )
            });

        let subject = msg
            .as_ref()
            .and_then(|m| m.subject.clone())
            .unwrap_or_else(|| "(no subject)".to_string());

        let date = msg
            .as_ref()
            .map(|m| format_relative_time(m.received_ts))
            .unwrap_or_default();

        let node = ThreadNode::new(&subject, &from, &date);
        node.set_body_preview(body);

        for child in &t.children {
            let child_node = Self::build_node(child, maildir);
            node.add_child(&child_node);
        }

        node
    }
}

/// Format a Unix timestamp as a human-readable relative time string
/// (e.g. "3d ago", "2h ago", "1w ago").
fn format_relative_time(ts: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let diff = now - ts;
    if diff < 0 {
        return "just now".to_string();
    }
    let mins = diff / 60;
    let hours = diff / 3600;
    let days = diff / 86400;
    let weeks = diff / (7 * 86400);
    if weeks > 0 {
        format!("{}w ago", weeks)
    } else if days > 0 {
        format!("{}d ago", days)
    } else if hours > 0 {
        format!("{}h ago", hours)
    } else if mins > 0 {
        format!("{}m ago", mins)
    } else {
        "just now".to_string()
    }
}