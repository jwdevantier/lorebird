//! Lua VM integration for loreread.
//!
//! Wraps mlua to provide:
//! - Configuration loading (profiles, views, hooks)
//! - Hook dispatch (fetch, send, reply-template)
//! - API surface exposed to Lua scripts

use mlua::{Lua, Result as LuaResult, Table};

/// A Lua VM configured with the loreread API.
pub struct Vm {
    lua: Lua,
}

/// Profile configuration as loaded from Lua.
#[derive(Debug, Clone)]
pub struct ProfileConfig {
    pub name: String,
    pub maildir: String,
}

impl Vm {
    /// Create a new Lua VM and register loreread API functions.
    pub fn new() -> LuaResult<Self> {
        let lua = Lua::new();
        // TODO: register API functions on the Lua globals table
        Ok(Self { lua })
    }

    /// Evaluate a Lua expression and return the result as a string.
    pub fn eval(&self, code: &str) -> LuaResult<String> {
        let val: mlua::Value = self.lua.load(code).eval()?;
        Ok(format!("{:?}", val))
    }

    /// Load the global config table from the Lua VM.
    ///
    /// Expects a global `loreread` table with configuration.
    /// Returns `None` if no config is defined.
    pub fn load_config(&self) -> LuaResult<Option<Table>> {
        let globals = self.lua.globals();
        if let Ok(config) = globals.get::<Table>("loreread") {
            Ok(Some(config))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_vm_works() {
        let vm = Vm::new().unwrap();
        assert!(vm.load_config().unwrap().is_none());
    }

    #[test]
    fn eval_simple_expression() {
        let vm = Vm::new().unwrap();
        let result = vm.eval("2 + 2").unwrap();
        assert!(result.contains("4"));
    }
}
