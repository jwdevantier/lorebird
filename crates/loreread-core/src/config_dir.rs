//! OS-specific configuration directory resolution.
//!
//! Returns the standard directory for application configuration files
//! on each platform:
//!
//! | Platform | Directory |
//! |----------|----------|
//! | Linux/BSD | `$XDG_CONFIG_HOME` or `$HOME/.config` |
//! | macOS | `$HOME/Library/Application Support` |
//! | Windows | `%APPDATA%` |
//!
//! Follows the [XDG Base Directory Specification] on Linux and
//! platform conventions elsewhere.
//!
//! [XDG Base Directory Specification]: https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html

use std::path::PathBuf;

/// Return the OS-standard configuration directory.
///
/// On Linux/BSD this honours `$XDG_CONFIG_HOME` and falls back to
/// `$HOME/.config`.  On macOS it returns `~/Library/Application Support`.
/// On Windows it returns `%APPDATA%`.
///
/// Returns `None` when the relevant environment variable is unset or empty.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().filter(|s| !s.is_empty()).map(PathBuf::from)
    }

    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| h.join("Library/Application Support"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Some(PathBuf::from(xdg));
            }
        }
        home_dir().map(|h| h.join(".config"))
    }
}

/// Return the full path to loreread's config directory (without filename).
///
/// | Platform | Path |
/// |----------|------|
/// | Linux/BSD | `~/.config/loreread/` |
/// | macOS | `~/Library/Application Support/loreread/` |
/// | Windows | `%APPDATA%\loreread\` |
///
/// Returns `None` when the OS config directory cannot be determined.
pub fn loreread_confdir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("loreread"))
}

/// Return the full path to loreread's config file.
///
/// | Platform | Path |
/// |----------|------|
/// | Linux/BSD | `~/.config/loreread/config.lua` |
/// | macOS | `~/Library/Application Support/loreread/config.lua` |
/// | Windows | `%APPDATA%\loreread\config.lua` |
///
/// Returns `None` when the OS config directory cannot be determined.
pub fn loreread_conf_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("loreread").join("config.lua"))
}

// ── Unix/macOS home directory ─────────────────────────────────────────

#[cfg(not(target_os = "windows"))]
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_returns_something() {
        let dir = config_dir();
        assert!(dir.is_some(), "config_dir() returned None — cannot determine config directory");
        let path = dir.unwrap();
        assert!(path.is_absolute(), "config_dir() must return an absolute path, got {:?}", path);
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn unix_xdg_override() {
        if std::env::var("XDG_CONFIG_HOME").is_ok() {
            let dir = config_dir().unwrap();
            let xdg = std::env::var("XDG_CONFIG_HOME").unwrap();
            assert_eq!(dir, PathBuf::from(xdg));
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_uses_application_support() {
        let dir = config_dir().unwrap();
        assert!(dir.to_string_lossy().contains("Application Support"));
    }

    #[test]
    fn loreread_conf_path_returns_something() {
        let path = loreread_conf_path();
        assert!(path.is_some(), "loreread_conf_path() returned None");
        let path = path.unwrap();
        assert!(path.is_absolute(), "loreread_conf_path() must return an absolute path, got {:?}", path);
        assert!(path.ends_with("loreread/config.lua"),
            "loreread_conf_path() should end with loreread/config.lua, got {:?}", path);
    }

    #[test]
    fn loreread_confdir_returns_something() {
        let dir = loreread_confdir();
        assert!(dir.is_some(), "loreread_confdir() returned None");
        let dir = dir.unwrap();
        assert!(dir.is_absolute(), "loreread_confdir() must return an absolute path, got {:?}", dir);
        assert!(dir.ends_with("loreread"),
            "loreread_confdir() should end with loreread, got {:?}", dir);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_uses_appdata() {
        let dir = config_dir().unwrap();
        let appdata = std::env::var("APPDATA").unwrap();
        assert_eq!(dir, PathBuf::from(appdata));
    }
}