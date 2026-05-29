//! Lua VM integration for loreread.
//!
//! Wraps mlua to provide:
//! - Configuration loading (profiles, views, hooks)
//! - `sh()` API helper for running external commands
//! - Hook dispatch (fetch, reply, send)

mod config;

pub use config::{AppConfig, GlobalHooks, LoadedConfig, ProfileData, ProfileHooks, ResolvedProfile, UserInfo, ViewConfig};
pub use loreread_core::compose::{ComposeMail, ParentMail};

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{Lua, LuaSerdeExt, Result as LuaResult, Table, Value};
use mlua::{DeserializeOptions, IntoLua};

/// Temp-file counter for unique filenames.
static TMPFILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A Lua VM configured with the loreread API.
pub struct Vm {
    lua: Lua,
    /// Paths to temporary files created by `write_tmpfile`,
    /// cleaned up when the Vm is dropped.
    temp_files: std::cell::RefCell<Vec<std::path::PathBuf>>,
}

impl Vm {
    /// Create a new Lua VM and register the loreread API surface.
    ///
    /// The following globals are available to Lua scripts:
    ///
    /// - `sh(cmd)` — run an external command, return result table
    /// - `read_file(path)` — read a file's contents as a string
    pub fn new() -> LuaResult<Self> {
        let lua = Lua::new();
        let vm = Self { lua, temp_files: std::cell::RefCell::new(Vec::new()) };
        vm.register_helpers()?;
        Ok(vm)
    }

    // ── API helpers ─────────────────────────────────────────────

    /// Register loreread API functions as Lua globals.
    fn register_helpers(&self) -> LuaResult<()> {
        // ── sh(cmd) → { ok, exit_code, stdout, stderr } ────────
        let sh_fn = self.lua.create_function(|lua, args: Table| {
            let cmd_args: Vec<String> = args
                .sequence_values::<String>()
                .collect::<Result<Vec<_>, _>>()?;

            if cmd_args.is_empty() {
                let err_table = lua.create_table()?;
                err_table.set("ok", false)?;
                err_table.set("exit_code", -1)?;
                err_table.set("stdout", String::new())?;
                err_table.set("stderr", "sh: empty command".to_string())?;
                return Ok(err_table);
            }

            let output = match std::process::Command::new(&cmd_args[0])
                .args(&cmd_args[1..])
                .output()
            {
                Ok(o) => o,
                Err(e) => {
                    let err_table = lua.create_table()?;
                    err_table.set("ok", false)?;
                    err_table.set("exit_code", -1)?;
                    err_table.set("stdout", String::new())?;
                    err_table.set("stderr", format!("sh: {}", e))?;
                    return Ok(err_table);
                }
            };

            let result = lua.create_table()?;
            result.set("ok", output.status.success())?;
            result.set("exit_code", output.status.code().unwrap_or(-1))?;
            result.set(
                "stdout",
                String::from_utf8_lossy(&output.stdout).to_string(),
            )?;
            result.set(
                "stderr",
                String::from_utf8_lossy(&output.stderr).to_string(),
            )?;
            Ok(result)
        })?;

        self.lua.globals().set("sh", sh_fn)?;

        // ── read_file(path) → string ────────────────────────────
        let read_file_fn = self.lua.create_function(|_, path: String| {
            std::fs::read_to_string(&path).map_err(|e| {
                mlua::Error::external(format!(
                    "read_file: cannot read '{}': {}",
                    path, e
                ))
            })
        })?;

        self.lua.globals().set("read_file", read_file_fn)?;

        // ── write_tmpfile(content) → path ──────────────────────────
        //   Writes `content` to a unique temporary file and returns
        //   the path.  Files are cleaned up when the Vm is dropped.
        let write_tmpfile_fn = self.lua.create_function(|_lua, content: String| {
            let count = TMPFILE_COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let name = format!("loreread_{}_{}", pid, count);
            let path = std::env::temp_dir().join(name);
            std::fs::write(&path, &content).map_err(|e| {
                mlua::Error::external(format!("write_tmpfile: {}", e))
            })?;
            Ok(path.to_string_lossy().to_string())
        })?;

        self.lua.globals().set("write_tmpfile", write_tmpfile_fn)?;

        // ── mail_to_rfc2822(mail_table) → string ────────────────────
        //   Converts a mail table (same structure as passed to on_reply /
        //   on_send) to an RFC 2822 formatted string.
        let mail_to_rfc2822_fn = self.lua.create_function(|_lua, table: Table| {
            let mail = extract_compose_mail_from_table(&table)?;
            Ok(mail.to_rfc2822())
        })?;

        self.lua.globals().set("mail_to_rfc2822", mail_to_rfc2822_fn)?;

        Ok(())
    }

    // ── Config loading ──────────────────────────────────────────

    /// Load a config file from disk, execute it, and return the
    /// parsed configuration (data) together with extracted hook handles.
    ///
    /// The Lua script must either **return** a config table or set a
    /// global `config` variable.
    pub fn load_config_file(&self, path: &Path) -> LuaResult<LoadedConfig> {
        let code = std::fs::read_to_string(path).map_err(|e| {
            mlua::Error::external(format!(
                "cannot read config file '{}': {}",
                path.display(),
                e
            ))
        })?;
        self.load_config_string(&code)
    }

    /// Execute a Lua config string and return the parsed configuration.
    ///
    /// The string must either **return** a config table (idiomatic Lua
    /// module pattern) or set a global `config` variable (legacy style).
    /// Both forms are supported:
    ///
    /// ```lua
    /// -- Modern style (return a table):
    /// return {
    ///   profiles = { ... },
    ///   on_reply = function(...) ... end,
    /// }
    ///
    /// -- Legacy style (set global):
    /// config = {
    ///   profiles = { ... },
    /// }
    /// ```
    pub fn load_config_string(&self, code: &str) -> LuaResult<LoadedConfig> {
        // ── Try return-style first, fall back to global ─────
        let config_table: Table = match self.lua.load(code).eval() {
            Ok(Value::Table(t)) => t,
            Ok(_) => {
                return Err(mlua::Error::external(
                    "Config script returned a non-table value",
                ));
            }
            Err(_) => {
                // Not an expression — likely the legacy `config = { ... }` style.
                // Execute as a statement block and read from globals.
                self.lua.load(code).exec()?;
                self.lua.globals().get("config").map_err(|_| {
                    mlua::Error::external(
                        "Config file must either return a table or set a "
                            .to_string()
                        + "global 'config' table.\n"
                        + "\n"
                        + "Modern style (return a table):\n"
                        + "  return {\n"
                        + "    profiles = { ... },\n"
                        + "  }\n"
                        + "\n"
                        + "Legacy style (set global):\n"
                        + "  config = {\n"
                        + "    profiles = { ... },\n"
                        + "  }",
                    )
                })?
            }
        };

        // ── Extract hook handles (before serde deserialisation) ──
        // Global hooks
        let on_reply: Option<mlua::Function> = config_table.get("on_reply")?;
        let on_send: Option<mlua::Function> = config_table.get("on_send")?;

        // Per-profile hooks
        let profiles_table: Table = config_table.get("profiles")?;
        let mut profile_hooks = HashMap::new();
        for pair in profiles_table.pairs::<String, Table>() {
            let (key, profile_table) = pair?;
            let on_fetch: Option<mlua::Function> = profile_table.get("on_fetch")?;
            profile_hooks.insert(key, ProfileHooks { on_fetch });
        }

        // ── Deserialise data portion ─────────────────────────
        // Lua tables may contain function values (hooks) which serde
        // cannot represent, so we must tell the deserialiser to skip
        // unsupported types.
        let config_value: Value = config_table.into_lua(&self.lua)?;
        let options = DeserializeOptions::new().deny_unsupported_types(false);
        let app_config: AppConfig = self.lua.from_value_with(config_value, options)?;

        Ok(LoadedConfig {
            config: app_config,
            profile_hooks,
            global_hooks: GlobalHooks { on_reply, on_send },
        })
    }

    // ── Hook calling ────────────────────────────────────────────

    /// Call the on_fetch hook for a profile.
    ///
    /// Returns `Ok(true)` if the hook executed and returned a truthy
    /// value, `Ok(false)` if the hook returned falsy, or `Err` if
    /// the hook does not exist or failed.
    pub fn call_on_fetch(
        &self,
        profile_label: &str,
        hooks: &ProfileHooks,
    ) -> LuaResult<bool> {
        let func = hooks.on_fetch.as_ref().ok_or_else(|| {
            mlua::Error::external(format!(
                "profile '{}' has no on_fetch hook",
                profile_label
            ))
        })?;

        // Lua truthiness: nil/false → false, everything else → true
        let result: bool = func.call(profile_label)?;
        Ok(result)
    }

    /// Call the optional `on_reply` hook.
    ///
    /// Passes `(profile_label, parent_table, mail_table)`.
    /// The hook may modify `mail` in-place or return a new table.
    /// Either way, the (potentially modified) mail is extracted and
    /// returned.
    pub fn call_on_reply(
        &self,
        func: &mlua::Function,
        profile_label: &str,
        parent: &ParentMail,
        mail: &ComposeMail,
    ) -> LuaResult<ComposeMail> {
        let parent_table = build_parent_table(&self.lua, parent)?;
        let mail_table = build_mail_table(&self.lua, mail)?;

        // Call the hook; discard the return value.  The user can
        // modify `mail` in-place — we always extract from the table
        // argument after the call.
        let _: mlua::Value = func.call((profile_label, parent_table, &mail_table))?;

        let modified = extract_compose_mail_from_table(&mail_table)?;
        Ok(modified)
    }

    /// Call the required `on_send` hook.
    ///
    /// Passes `(profile_label, mail_table)`.  The hook can use
    /// `mail_to_rfc2822()` and `write_tmpfile()` to format and
    /// deliver the mail.
    pub fn call_on_send(
        &self,
        func: &mlua::Function,
        profile_label: &str,
        mail: &ComposeMail,
    ) -> LuaResult<()> {
        let mail_table = build_mail_table(&self.lua, mail)?;
        func.call::<()>((profile_label, mail_table))?;
        Ok(())
    }

    // ── Legacy ──────────────────────────────────────────────────

    /// Evaluate a Lua expression and return the result as a string.
    pub fn eval(&self, code: &str) -> LuaResult<String> {
        let val: mlua::Value = self.lua.load(code).eval()?;
        Ok(format!("{:?}", val))
    }

    /// Clean up temporary files created by `write_tmpfile`.
    pub fn cleanup_temp_files(&self) {
        let paths = self.temp_files.borrow_mut().drain(..).collect::<Vec<_>>();
        for path in paths {
            let _ = std::fs::remove_file(&path);
        }
    }
}

impl Drop for Vm {
    fn drop(&mut self) {
        self.cleanup_temp_files();
    }
}

// ── Lua ↔ Rust conversion helpers ─────────────────────────────────────

/// Build a Lua table from a `ParentMail` struct.
fn build_parent_table(lua: &Lua, parent: &ParentMail) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set("message_id", parent.message_id.as_deref().unwrap_or(""))?;
    table.set("from", parent.from.as_str())?;
    table.set("to", parent.to.as_str())?;
    table.set("cc", parent.cc.as_str())?;
    table.set("subject", parent.subject.as_str())?;
    table.set("date", parent.date.as_str())?;
    table.set("references", parent.references.as_str())?;
    table.set("in_reply_to", parent.in_reply_to.as_deref().unwrap_or(""))?;
    table.set("body_text", parent.body_text.as_str())?;

    // All original headers from the message on disk.
    let headers = lua.create_table()?;
    for (k, v) in &parent.headers {
        headers.set(k.as_str(), v.as_str())?;
    }
    table.set("headers", headers)?;

    Ok(table)
}

/// Build a Lua table from a `ComposeMail` struct.
fn build_mail_table(lua: &Lua, mail: &ComposeMail) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set("from", mail.from.as_str())?;
    table.set("to", mail.to.as_str())?;
    table.set("cc", mail.cc.as_str())?;
    table.set("bcc", mail.bcc.as_str())?;
    table.set("subject", mail.subject.as_str())?;

    if let Some(ref v) = mail.date {
        table.set("date", v.as_str())?;
    }
    if let Some(ref v) = mail.message_id {
        table.set("message_id", v.as_str())?;
    }
    if let Some(ref v) = mail.in_reply_to {
        table.set("in_reply_to", v.as_str())?;
    }
    if let Some(ref v) = mail.references {
        table.set("references", v.as_str())?;
    }

    table.set("body_text", mail.body_text.as_str())?;

    // Headers sub-table
    let headers = lua.create_table()?;
    for (k, v) in &mail.headers {
        headers.set(k.as_str(), v.as_str())?;
    }
    table.set("headers", headers)?;

    Ok(table)
}

/// Extract a `ComposeMail` from a Lua table.
///
/// Missing optional fields default to empty strings or None.
fn extract_compose_mail_from_table(table: &Table) -> LuaResult<ComposeMail> {
    let from: String = table.get("from")?;
    let to: String = table.get("to")?;
    let cc: String = table.get("cc").unwrap_or_default();
    let bcc: String = table.get("bcc").unwrap_or_default();
    let subject: String = table.get("subject")?;
    let date: Option<String> = table.get("date")?;
    let message_id: Option<String> = table.get("message_id")?;
    let in_reply_to: Option<String> = table.get("in_reply_to")?;
    let references: Option<String> = table.get("references")?;
    let body_text: String = table.get("body_text")?;

    let headers: HashMap<String, String> = match table.get::<Table>("headers") {
        Ok(h) => {
            let mut map = HashMap::new();
            for pair in h.pairs::<String, String>() {
                let (k, v) = pair?;
                map.insert(k, v);
            }
            map
        }
        Err(_) => HashMap::new(),
    };

    Ok(ComposeMail {
        from,
        to,
        cc,
        bcc,
        subject,
        date,
        message_id,
        in_reply_to,
        references,
        body_text,
        headers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config() -> &'static str {
        r#"
config = {
  profiles = {
    ["test"] = {
      maildir = "/tmp/test-mail",
      on_fetch = function(label)
        return true
      end,
    },
  },
}
"#
    }

    #[test]
    fn new_vm_works() {
        let vm = Vm::new().unwrap();
        let result = vm.eval("2 + 2").unwrap();
        assert!(result.contains("4"));
    }

    #[test]
    fn load_minimal_config() {
        let vm = Vm::new().unwrap();
        let loaded = vm.load_config_string(minimal_config()).unwrap();

        assert!(loaded.config.user.is_none());
        assert_eq!(loaded.config.profiles.len(), 1);

        let profile = &loaded.config.profiles["test"];
        assert_eq!(profile.maildir, "/tmp/test-mail");
        assert!(profile.name.is_none());
        assert!(profile.email.is_none());
        assert!(profile.views.is_empty());

        // Hook was extracted
        assert!(loaded.profile_hooks["test"].on_fetch.is_some());
    }

    #[test]
    fn load_config_return_style() {
        let vm = Vm::new().unwrap();
        let code = r#"
return {
  profiles = {
    ["test"] = {
      maildir = "/tmp/test-mail",
      on_fetch = function(label) return true end,
    },
  },
}
"#;
        let loaded = vm.load_config_string(code).unwrap();
        assert_eq!(loaded.config.profiles.len(), 1);
        assert!(loaded.profile_hooks["test"].on_fetch.is_some());
    }

    #[test]
    fn load_full_config() {
        let vm = Vm::new().unwrap();
        let code = r#"
config = {
  user = {
    name = "Riccardo Maffulli",
    email = "riccardo@defmacro.it",
  },
  profiles = {
    ["qemu nvme"] = {
      name = "Riccardo (work)",
      email = "riccardo@work.com",
      maildir = "/home/nixos/loremail/INBOX",
      on_fetch = function(label)
        return sh({"echo", "fetching"}).ok
      end,
      views = {
        { label = "last week", query = "date:1w.." },
        { label = "maintainer", query = "f:its@irrelevant.dk" },
      },
    },
    ["personal"] = {
      -- inherits global name/email
      maildir = "/home/nixos/loremail/personal",
      on_fetch = function(label)
        return true
      end,
    },
  },
  on_send = function(profile, mail_fpath)
    sh({"sendmail", "-t"})
  end,
}
"#;
        let loaded = vm.load_config_string(code).unwrap();

        // Global user
        let user = loaded.config.user.as_ref().unwrap();
        assert_eq!(user.name.as_deref(), Some("Riccardo Maffulli"));
        assert_eq!(user.email.as_deref(), Some("riccardo@defmacro.it"));

        // Profile data
        assert_eq!(loaded.config.profiles.len(), 2);

        let qemu = &loaded.config.profiles["qemu nvme"];
        assert_eq!(qemu.name.as_deref(), Some("Riccardo (work)"));
        assert_eq!(qemu.email.as_deref(), Some("riccardo@work.com"));
        assert_eq!(qemu.maildir, "/home/nixos/loremail/INBOX");
        assert_eq!(qemu.views.len(), 2);
        assert_eq!(qemu.views[0].label, "last week");
        assert_eq!(qemu.views[0].query, "date:1w..");

        let personal = &loaded.config.profiles["personal"];
        assert!(personal.name.is_none());
        assert!(personal.email.is_none());
        assert!(personal.views.is_empty());

        // Resolution
        let resolved = loaded.config.resolve_all();
        assert_eq!(resolved["qemu nvme"].name, "Riccardo (work)");
        assert_eq!(resolved["qemu nvme"].email, "riccardo@work.com");
        assert_eq!(resolved["personal"].name, "Riccardo Maffulli");
        assert_eq!(resolved["personal"].email, "riccardo@defmacro.it");

        // Hooks
        assert!(loaded.profile_hooks["qemu nvme"].on_fetch.is_some());
        assert!(loaded.profile_hooks["personal"].on_fetch.is_some());
        assert!(loaded.global_hooks.on_send.is_some());
        assert!(loaded.global_hooks.on_reply.is_none());
    }

    #[test]
    fn resolve_cascade() {
        let vm = Vm::new().unwrap();
        let loaded = vm.load_config_string(minimal_config()).unwrap();
        let resolved = loaded.config.resolve_all();

        // No global user, no local name/email → defaults
        assert_eq!(resolved["test"].name, "Anonymous");
        assert_eq!(resolved["test"].email, "unknown@localhost");
    }

    #[test]
    fn sh_helper_works() {
        let vm = Vm::new().unwrap();
        let result: Table = vm
            .lua
            .load(r#"return sh({"echo", "hello"})"#)
            .eval()
            .unwrap();
        assert_eq!(result.get::<bool>("ok").unwrap(), true);
        assert_eq!(result.get::<i64>("exit_code").unwrap(), 0);
        assert!(result.get::<String>("stdout").unwrap().contains("hello"));
    }

    #[test]
    fn sh_helper_captures_error() {
        let vm = Vm::new().unwrap();
        let result: Table = vm
            .lua
            .load(r#"return sh({"sh", "-c", "exit 42"})"#)
            .eval()
            .unwrap();
        assert_eq!(result.get::<bool>("ok").unwrap(), false);
        assert_eq!(
            result.get::<Option<i64>>("exit_code").unwrap(),
            Some(42)
        );
    }

    #[test]
    fn sh_helper_handles_missing_command() {
        let vm = Vm::new().unwrap();
        let result: Table = vm
            .lua
            .load(r#"return sh({"nonexistent_command_xyz_123"})"#)
            .eval()
            .unwrap();
        assert_eq!(result.get::<bool>("ok").unwrap(), false);
        assert_eq!(result.get::<i64>("exit_code").unwrap(), -1);
    }

    #[test]
    fn read_file_helper() {
        let vm = Vm::new().unwrap();
        let tmp = std::env::temp_dir().join("loreread_test_read_file.txt");
        std::fs::write(&tmp, "hello from file").unwrap();
        let code = format!(
            r#"return read_file("{}")"#,
            tmp.display()
        );
        let content: String = vm.lua.load(&code).eval().unwrap();
        assert_eq!(content, "hello from file");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn call_on_fetch_returns_truthy() {
        let vm = Vm::new().unwrap();
        let loaded = vm.load_config_string(minimal_config()).unwrap();
        let hooks = &loaded.profile_hooks["test"];
        let result = vm.call_on_fetch("test", hooks).unwrap();
        assert!(result);
    }

    #[test]
    fn call_on_fetch_with_falsy_hook() {
        let vm = Vm::new().unwrap();
        let code = r#"
config = {
  profiles = {
    ["fail"] = {
      maildir = "/tmp/none",
      on_fetch = function(label)
        return false
      end,
    },
  },
}
"#;
        let loaded = vm.load_config_string(code).unwrap();
        let hooks = &loaded.profile_hooks["fail"];
        let result = vm.call_on_fetch("fail", hooks).unwrap();
        assert!(!result);
    }

    #[test]
    fn write_tmpfile_helper() {
        let vm = Vm::new().unwrap();
        let content = "Hello, tmpfile!";
        let path: String = vm.lua
            .load(&format!(r#"return write_tmpfile("{}")"#, content.replace('\n', "\\n")))
            .eval()
            .unwrap();
        assert!(!path.is_empty());
        let read_back = std::fs::read_to_string(&path).unwrap();
        assert_eq!(read_back, content);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn mail_to_rfc2822_helper() {
        let vm = Vm::new().unwrap();
        let result: String = vm.lua
            .load(r#"
local mail = {
  from = "Alice <alice@example.com>",
  to = "Bob <bob@example.com>",
  cc = "",
  bcc = "",
  subject = "Test",
  in_reply_to = "<parent@example.com>",
  references = "<grandparent@example.com> <parent@example.com>",
  body_text = "Hello\n",
  headers = { ["X-Mailer"] = "loreread" },
}
return mail_to_rfc2822(mail)
"#)
            .eval()
            .unwrap();
        assert!(result.contains("From: Alice <alice@example.com>"));
        assert!(result.contains("To: Bob <bob@example.com>"));
        assert!(result.contains("Subject: Test"));
        assert!(result.contains("In-Reply-To: <parent@example.com>"));
        assert!(result.contains("X-Mailer: loreread"));
        assert!(result.contains("Hello"));
        // Auto-generated headers
        assert!(result.contains("Date: "));
        assert!(result.contains("Message-ID: <"));
        assert!(result.contains("MIME-Version: 1.0"));
        assert!(result.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(result.contains("X-Mailer: loreread"));
    }

    #[test]
    fn call_on_reply_returns_modified_mail() {
        let vm = Vm::new().unwrap();
        let code = r#"
config = {
  profiles = {
    ["test"] = {
      maildir = "/tmp/test-mail",
      on_fetch = function(label) return true end,
    },
  },
  on_reply = function(profile, parent, mail)
    mail.cc = "added-by-hook@example.com"
    return mail
  end,
}
"#;
        let loaded = vm.load_config_string(code).unwrap();
        let on_reply = loaded.global_hooks.on_reply.as_ref().unwrap();

        let parent = loreread_core::compose::ParentMail {
            message_id: Some("<abc@def>".to_string()),
            from: "Alice <alice@example.com>".to_string(),
            to: "list@example.com".to_string(),
            cc: String::new(),
            subject: "Test subject".to_string(),
            date: "2024-01-15".to_string(),
            references: String::new(),
            in_reply_to: None,
            body_text: "Original body".to_string(),
            headers: std::collections::HashMap::new(),
        };
        let mail = loreread_core::compose::ComposeMail::new_reply(
            &parent, "Riccardo", "riccardo@defmacro.it",
        );

        let result = vm.call_on_reply(on_reply, "test", &parent, &mail).unwrap();
        assert_eq!(result.cc, "added-by-hook@example.com");
        assert_eq!(result.to, "list@example.com, Riccardo <riccardo@defmacro.it>"); // unchanged by hook
    }

    #[test]
    fn call_on_reply_in_place_modification() {
        let vm = Vm::new().unwrap();
        let code = r#"
config = {
  profiles = {
    ["test"] = {
      maildir = "/tmp/test-mail",
      on_fetch = function(label) return true end,
    },
  },
  on_reply = function(profile, parent, mail)
    -- modify in-place, no explicit return needed
    mail.cc = "added-in-place@example.com"
  end,
}
"#;
        let loaded = vm.load_config_string(code).unwrap();
        let on_reply = loaded.global_hooks.on_reply.as_ref().unwrap();

        let parent = loreread_core::compose::ParentMail {
            message_id: Some("<abc@def>".to_string()),
            from: "Alice <alice@example.com>".to_string(),
            to: String::new(),
            cc: String::new(),
            subject: "Hello".to_string(),
            date: String::new(),
            references: String::new(),
            in_reply_to: None,
            body_text: String::new(),
            headers: std::collections::HashMap::new(),
        };
        let mail = loreread_core::compose::ComposeMail::new_reply(
            &parent, "Bob", "bob@example.com",
        );

        // The hook modifies mail.cc in-place; we always get back the
        // (potentially modified) mail, never None.
        let result = vm.call_on_reply(on_reply, "test", &parent, &mail).unwrap();
        assert_eq!(result.cc, "added-in-place@example.com");
    }

    #[test]
    fn call_on_send_invokes_hook() {
        let vm = Vm::new().unwrap();
        // Use a write_tmpfile-based on_send that we can verify
        let tmp = std::env::temp_dir().join("loreread_test_on_send.txt");
        let tmp_path = tmp.display().to_string();
        let code = format!(r#"
config = {{
  profiles = {{
    ["test"] = {{
      maildir = "/tmp/test-mail",
      on_fetch = function(label) return true end,
    }},
  }},
  on_send = function(profile, mail)
    local fpath = write_tmpfile(mail_to_rfc2822(mail))
    -- Copy to a known location for the test to check
    sh({{"cp", fpath, "{}"}})
  end,
}}
"#, tmp_path);
        let loaded = vm.load_config_string(&code).unwrap();
        let on_send = loaded.global_hooks.on_send.as_ref().unwrap();

        let mail = loreread_core::compose::ComposeMail {
            from: "Alice <alice@example.com>".to_string(),
            to: "Bob <bob@example.com>".to_string(),
            cc: String::new(),
            bcc: String::new(),
            subject: "Test send".to_string(),
            date: None,
            message_id: None,
            in_reply_to: None,
            references: None,
            body_text: "Hello from send test\n".to_string(),
            headers: std::collections::HashMap::new(),
        };

        // The on_send should succeed without error
        vm.call_on_send(on_send, "test", &mail).unwrap();

        // Verify the file was written
        if let Ok(content) = std::fs::read_to_string(&tmp) {
            assert!(content.contains("From: Alice <alice@example.com>"));
            assert!(content.contains("Subject: Test send"));
            assert!(content.contains("Hello from send"));
            std::fs::remove_file(&tmp).ok();
        }
    }
}