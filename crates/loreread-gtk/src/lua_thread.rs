//! Lua thread — runs the VM in a dedicated background thread.
//!
//! The Lua thread *creates* and *owns* `Vm` and `LoadedConfig`.  Neither
//! ever crosses a thread boundary.  The main (GTK) thread communicates
//! with it via `mpsc` channels, sending plain-data `LuaCommand`s and
//! receiving `LuaResult`s — all `Send`-compatible.
//!
//! See `specs/threading.md` for the full architecture.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

use loreread_lua::{LoadedConfig, ResolvedProfile, Vm};

// ── Command protocol (main → Lua thread) ────────────────────────────

/// Commands sent from the main thread to the Lua thread.
#[derive(Debug)]
pub enum LuaCommand {
    /// Call `on_fetch` for the given profile.
    /// If the hook returns truthy, also index the maildir.
    Fetch {
        profile_label: String,
        maildir: PathBuf,
    },

    /// Shut down the Lua thread.
    Shutdown,
}

/// Results sent from the Lua thread back to the main thread.
#[derive(Debug)]
pub enum LuaResult {
    /// Config loaded successfully; here are the resolved profiles.
    InitDone {
        profiles: HashMap<String, ResolvedProfile>,
        theme: String,
        ui_scale: f64,
    },

    /// Config loading failed.
    InitFailed {
        error: String,
    },

    /// Fetch + index operation completed.
    FetchDone {
        profile_label: String,
        indexed_count: usize,
        error: Option<String>,
    },
}

// ── Lua thread state ────────────────────────────────────────────────

/// State kept inside the Lua thread (not `Send`, stays in that thread).
struct LuaState {
    vm: Vm,
    config: LoadedConfig,
    profiles: HashMap<String, ResolvedProfile>,
}

/// Initialisation result from the Lua thread.
pub struct InitResult {
    pub profiles: HashMap<String, ResolvedProfile>,
    pub theme: String,
    pub ui_scale: f64,
}

// ── Lua thread handle ──────────────────────────────────────────────

/// Handle to the background Lua thread.
pub struct LuaThread {
    cmd_tx: mpsc::Sender<LuaCommand>,
    result_rx: mpsc::Receiver<LuaResult>,
}

impl LuaThread {
    /// Spawn the Lua thread.  It creates the VM, loads config, and
    /// sends `InitDone` (or `InitFailed`) back via the result channel.
    pub fn spawn(config_path: Option<PathBuf>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<LuaCommand>();
        let (result_tx, result_rx) = mpsc::channel::<LuaResult>();

        std::thread::Builder::new()
            .name("loreread-lua".into())
            .spawn(move || {
                lua_thread_main(config_path, cmd_rx, result_tx);
            })
            .expect("failed to spawn Lua thread");

        Self { cmd_tx, result_rx }
    }

    /// Block until the Lua thread has loaded config.
    pub fn recv_init(&self) -> Result<InitResult, String> {
        match self.result_rx.recv() {
            Ok(LuaResult::InitDone { profiles, theme, ui_scale }) => Ok(InitResult { profiles, theme, ui_scale }),
            Ok(LuaResult::InitFailed { error }) => Err(error),
            Ok(_) => Err("unexpected result from Lua thread".to_string()),
            Err(_) => Err("Lua thread disconnected during init".to_string()),
        }
    }

    /// Send a command to the Lua thread.
    pub fn send(&self, cmd: LuaCommand) -> Result<(), mpsc::SendError<LuaCommand>> {
        self.cmd_tx.send(cmd)
    }

    /// Try to receive a result (non-blocking).
    pub fn try_recv(&self) -> Result<LuaResult, mpsc::TryRecvError> {
        self.result_rx.try_recv()
    }

    /// Tell the Lua thread to shut down.
    #[allow(dead_code)]
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(LuaCommand::Shutdown);
    }
}

// ── Lua thread main loop ───────────────────────────────────────────

fn lua_thread_main(
    config_path: Option<PathBuf>,
    cmd_rx: mpsc::Receiver<LuaCommand>,
    result_tx: mpsc::Sender<LuaResult>,
) {
    // 1. Create VM and load config (entirely within this thread)
    let state = match load_config(config_path) {
        Ok(state) => {
            eprintln!("[loreread-lua] loaded {} profile(s)", state.profiles.len());
            let profiles = state.profiles.clone();
            let theme = state.config.config.theme.clone();
            let ui_scale = state.config.config.ui_scale;
            let _ = result_tx.send(LuaResult::InitDone { profiles, theme, ui_scale });
            state
        }
        Err(e) => {
            eprintln!("[loreread-lua] config load failed: {}", e);
            let _ = result_tx.send(LuaResult::InitFailed { error: e });
            let vm = Vm::new().expect("failed to create Lua VM");
            LuaState {
                vm,
                config: empty_config(),
                profiles: HashMap::new(),
            }
        }
    };

    // 2. Command loop
    loop {
        match cmd_rx.recv() {
            Ok(LuaCommand::Fetch { profile_label, maildir }) => {
                eprintln!(
                    "[loreread-lua] Fetch: profile='{}' maildir='{}'",
                    profile_label,
                    maildir.display()
                );
                let t = std::time::Instant::now();
                let result = handle_fetch(&state, &profile_label, &maildir);
                eprintln!("[loreread-lua] Fetch complete in {:?}", t.elapsed());
                let _ = result_tx.send(result);
            }
            Ok(LuaCommand::Shutdown) | Err(_) => {
                eprintln!("[loreread-lua] thread shutting down");
                return;
            }
        }
    }
}

/// Load config inside the Lua thread.  Returns `LuaState`.
fn load_config(
    config_path: Option<PathBuf>,
) -> Result<LuaState, String> {
    let vm = Vm::new().map_err(|e| format!("failed to create Lua VM: {}", e))?;

    let loaded = match config_path {
        Some(ref path) => vm.load_config_file(path)
            .map_err(|e| format!("failed to load config from {}: {}", path.display(), e))?,
        None => {
            let default = dirs_for_loreread();
            let cfg_file = default.join("config.lua");
            if cfg_file.exists() {
                vm.load_config_file(&cfg_file)
                    .map_err(|e| format!("failed to load config from {}: {}", cfg_file.display(), e))?
            } else {
                return Err(format!("no config found at {}", cfg_file.display()));
            }
        }
    };

    let profiles = loaded.config.resolve_all();
    Ok(LuaState { vm, config: loaded, profiles })
}

/// Handle a Fetch command: call on_fetch, then index if truthy.
fn handle_fetch(
    state: &LuaState,
    profile_label: &str,
    maildir: &std::path::Path,
) -> LuaResult {
    // 1. Look up the profile hooks
    let hooks = state.config.profile_hooks.get(profile_label);

    // 2. Call on_fetch hook (if defined)
    if let Some(hooks) = hooks {
        eprintln!("[loreread-lua]   calling on_fetch for '{}'...", profile_label);
        match state.vm.call_on_fetch(profile_label, hooks) {
            Ok(true) => {
                eprintln!("[loreread-lua]   on_fetch returned true — indexing");
            }
            Ok(false) => {
                eprintln!("[loreread-lua]   on_fetch returned false — no new mail");
                return LuaResult::FetchDone {
                    profile_label: profile_label.to_string(),
                    indexed_count: 0,
                    error: None,
                };
            }
            Err(e) => {
                eprintln!("[loreread-lua]   on_fetch error: {}", e);
                return LuaResult::FetchDone {
                    profile_label: profile_label.to_string(),
                    indexed_count: 0,
                    error: Some(format!("on_fetch hook failed: {}", e)),
                };
            }
        }
    } else {
        eprintln!(
            "[loreread-lua]   no on_fetch hook for '{}' — indexing directly",
            profile_label
        );
    }

    // 3. Index the maildir
    eprintln!("[loreread-lua]   indexing '{}'...", maildir.display());
    let db_path = maildir.join(".loreread.db");
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            return LuaResult::FetchDone {
                profile_label: profile_label.to_string(),
                indexed_count: 0,
                error: Some(format!("cannot open database: {}", e)),
            };
        }
    };

    if let Err(e) = loreread_core::schema::init_db(&conn) {
        return LuaResult::FetchDone {
            profile_label: profile_label.to_string(),
            indexed_count: 0,
            error: Some(format!("cannot init schema: {}", e)),
        };
    }

    // Enable WAL for concurrent readers
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL;");

    match loreread_core::indexer::index_maildir(&conn, maildir) {
        Ok(n) => {
            eprintln!("[loreread-lua]   indexed {} message(s)", n);
            LuaResult::FetchDone {
                profile_label: profile_label.to_string(),
                indexed_count: n,
                error: None,
            }
        }
        Err(e) => LuaResult::FetchDone {
            profile_label: profile_label.to_string(),
            indexed_count: 0,
            error: Some(format!("indexing failed: {}", e)),
        },
    }
}

fn empty_config() -> LoadedConfig {
    use loreread_lua::{AppConfig, GlobalHooks};
    LoadedConfig {
        config: AppConfig { user: None, theme: "light".to_string(), ui_scale: 1.0, profiles: HashMap::new() },
        profile_hooks: HashMap::new(),
        global_hooks: GlobalHooks { on_reply: None, on_send: None },
    }
}

fn dirs_for_loreread() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        })
        .join("loreread")
}