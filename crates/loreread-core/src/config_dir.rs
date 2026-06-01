//! OS-specific configuration directory resolution.
//!
//! Returns the standard directory for application configuration files
//! on each platform:
//!
//! | Platform | Directory |
//! |----------|----------|
//! | Linux/BSD | `$XDG_CONFIG_HOME` or `$HOME/.config` |
//! | macOS | `$HOME/Library/Application Support` |
//! | Windows | `{FOLDERID_RoamingAppData}` (`%APPDATA%`) |
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
/// On Windows it returns `%APPDATA%` via `SHGetKnownFolderPath`.
///
/// Returns `None` only when the home directory cannot be determined
/// (e.g. `HOME` / `AppData` not set and OS APIs fail).
pub fn config_dir() -> Option<PathBuf> {
    // Keep this function public for callers that want the OS root only.
    _config_dir_impl()
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
    _config_dir_impl().map(|d| d.join("loreread").join("config.lua"))
}

/// Return the full path to loreread's config directory (without filename).
///
/// Returns `None` when the OS config directory cannot be determined.
pub fn loreread_confdir() -> Option<PathBuf> {
    _config_dir_impl().map(|d| d.join("loreread"))
}

fn _config_dir_impl() -> Option<PathBuf> {
    cfg_if::cfg_if! {
        if #[cfg(target_os = "windows")] {
            windows_config_dir()
        } else if #[cfg(target_os = "macos")] {
            macos_config_dir()
        } else {
            unix_config_dir()
        }
    }
}

// ── Linux / BSD ──────────────────────────────────────────────────────

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn unix_config_dir() -> Option<PathBuf> {
    // $XDG_CONFIG_HOME overrides everything.
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg));
        }
    }
    // Default: $HOME/.config
    home_dir().map(|h| h.join(".config"))
}

// ── macOS ─────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn macos_config_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join("Library/Application Support"))
}

// ── Windows ───────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn windows_config_dir() -> Option<PathBuf> {
    // Try %APPDATA% first — always set on normal Windows sessions.
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return Some(PathBuf::from(appdata));
        }
    }
    // Fall back to the known-folder API via SHGetKnownFolderPath.
    windows_known_folder(&windows::Win32::UI::Shell::FOLDERID_RoamingAppData)
}

#[cfg(target_os = "windows")]
fn windows_known_folder(
    rfid: &windows::core::GUID,
) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::SHGetKnownFolderPath;
    use windows::core::PCWSTR;

    let mut ptr = std::ptr::null_mut::<u16>();
    let hr = unsafe {
        SHGetKnownFolderPath(rfid, 0, None, &mut ptr)
    };
    if hr.is_ok() && !ptr.is_null() {
        let s = unsafe {
            let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
            let slice = std::slice::from_raw_parts(ptr, len);
            let s = String::from_utf16_lossy(slice);
            windows::Win32::System::Com::CoTaskMemFree(Some(ptr as *mut _));
            s
        };
        Some(PathBuf::from(s))
    } else {
        // Free on failure too (paranoid, but correct).
        if !ptr.is_null() {
            unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(ptr as *mut _)); }
        }
        None
    }
}

// ── Common ────────────────────────────────────────────────────────────

/// Determine the user's home directory.
///
/// Reads `$HOME` on Unix/macOS, falls back to `getpwuid_r` on failure.
/// On Windows this is not used (we go straight to `%APPDATA%`).
#[cfg(not(target_os = "windows"))]
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(pwuid_home)
}

#[cfg(not(target_os = "windows"))]
fn pwuid_home() -> Option<PathBuf> {
    // Safe fallback when $HOME is unset (e.g. daemons, containers).
    use std::ffi::CStr;
    let uid = unsafe { libc::getuid() };
    let mut buf = [0u8; 4096]; // large enough for struct passwd
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut::<libc::passwd>();

    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            pwd.as_mut_ptr(),
            buf.as_mut_ptr() as *mut i8,
            buf.len(),
            &mut result,
        )
    };
    if ret == 0 && !result.is_null() {
        let pwd = unsafe { pwd.assume_init() };
        if !pwd.pw_dir.is_null() {
            let c = unsafe { CStr::from_ptr(pwd.pw_dir) };
            return Some(PathBuf::from(c.to_string_lossy().into_owned()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_returns_something() {
        // On any reasonable CI/developer machine this should succeed.
        let dir = config_dir();
        assert!(dir.is_some(), "config_dir() returned None — cannot determine config directory");
        let path = dir.unwrap();
        assert!(path.is_absolute(), "config_dir() must return an absolute path, got {:?}", path);
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn unix_xdg_override() {
        // If XDG_CONFIG_HOME is set, config_dir() should use it.
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