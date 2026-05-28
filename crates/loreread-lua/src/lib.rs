//! Lua VM integration for loreread.
//!
//! Wraps mlua to provide:
//! - Configuration loading (profiles, views, hooks)
//! - `sh()` API helper for running external commands
//! - Hook dispatch (fetch, reply, send)

mod config;

pub use config::{AppConfig, GlobalHooks, LoadedConfig, ProfileData, ProfileHooks, ResolvedProfile, UserInfo, ViewConfig};

use std::collections::HashMap;
use std::path::Path;

use mlua::{Lua, LuaSerdeExt, Result as LuaResult, Table, Value};
use mlua::{DeserializeOptions, IntoLua};

/// A Lua VM configured with the loreread API.
pub struct Vm {
    lua: Lua,
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
        let vm = Self { lua };
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

        Ok(())
    }

    // ── Config loading ──────────────────────────────────────────

    /// Load a config file from disk, execute it, and return the
    /// parsed configuration (data) together with extracted hook handles.
    ///
    /// The Lua script must set a global `config` table.
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
    /// The string must set a global `config` table.
    pub fn load_config_string(&self, code: &str) -> LuaResult<LoadedConfig> {
        // ── Execute the config script ────────────────────────
        self.lua.load(code).exec()?;

        // ── Get the config table from globals ───────────────
        let config_table: Table = self.lua.globals().get("config").map_err(|_| {
            mlua::Error::external(
                "Config file must set a global 'config' table.\n\
                 Example:\n\
                 config = {\n\
                   profiles = { ... },\n\
                 }",
            )
        })?;

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

    // ── Legacy ──────────────────────────────────────────────────

    /// Evaluate a Lua expression and return the result as a string.
    pub fn eval(&self, code: &str) -> LuaResult<String> {
        let val: mlua::Value = self.lua.load(code).eval()?;
        Ok(format!("{:?}", val))
    }
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
}