//! Resolves where HeiGate keeps its local state (SQLite database, gateway
//! key). Mirrors the Go `internal/datadir` package one-to-one so both
//! implementations agree on paths during the migration.
//!
//! The location can be overridden with `DATA_DIR`; otherwise it follows the
//! platform convention for application support files.

use std::path::PathBuf;

/// The environment variable that overrides the data directory.
pub const ENV_VAR: &str = "DATA_DIR";

/// The folder name used under the platform's application support directory.
pub const APP_NAME: &str = "HeiGate";

/// Returns the data directory. It never returns an empty path; when the home
/// directory cannot be determined it falls back to a folder next to the
/// current working directory.
pub fn path() -> PathBuf {
    if let Some(dir) = std::env::var_os(ENV_VAR) {
        let dir = dir.to_string_lossy().trim().to_string();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }

    let home = home_dir();

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home {
            return home
                .join("Library")
                .join("Application Support")
                .join(APP_NAME);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata).join(APP_NAME);
            }
        }
        if let Some(home) = home {
            return home.join("AppData").join("Roaming").join(APP_NAME);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join(APP_NAME.to_lowercase());
            }
        }
        if let Some(home) = home {
            return home
                .join(".local")
                .join("share")
                .join(APP_NAME.to_lowercase());
        }
    }

    // `home` is unused on macOS when it resolved, and on Windows when APPDATA
    // resolved; keep the binding meaningful for all platforms.
    let _ = home;
    PathBuf::from(".").join("heigate-data")
}

/// Creates the data directory (0700) and returns it.
pub fn ensure() -> std::io::Result<PathBuf> {
    let dir = path();
    std::fs::create_dir_all(&dir)?;
    restrict_permissions(&dir);
    Ok(dir)
}

/// The SQLite file inside the data directory.
pub fn database_path() -> PathBuf {
    path().join("heigate.sqlite")
}

/// The gateway key file inside the data directory.
pub fn gateway_key_path() -> PathBuf {
    path().join("gateway.key")
}

/// Best-effort restriction of the directory to 0700 (owner only). No-op on
/// Windows, matching the Go implementation.
fn restrict_permissions(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

fn home_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            if !profile.is_empty() {
                return Some(PathBuf::from(profile));
            }
        }
    }
    None
}
