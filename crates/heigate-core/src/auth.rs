//! The local gateway credential. It lives outside the database and is
//! restricted to the current user, matching the Go `internal/api` auth code.

use std::fs;
use std::io;
use std::path::Path;

/// Prefix of every generated gateway key.
const KEY_PREFIX: &str = "hg_";

/// Reads the gateway key from `path`, creating it (0600) with a fresh random
/// value when the file is missing or empty.
pub fn load_or_create_gateway_key(path: &Path) -> io::Result<String> {
    match fs::read(path) {
        Ok(data) => {
            let key = String::from_utf8_lossy(&data).trim().to_string();
            if !key.is_empty() {
                restrict_to_owner(path)?;
                return Ok(key);
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).map_err(|err| io::Error::other(err.to_string()))?;
    let key = format!("{KEY_PREFIX}{}", hex_encode(&buf));

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    match options.open(path) {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(format!("{key}\n").as_bytes())?;
            Ok(key)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            // Another process created the key first; adopt its value.
            let data = fs::read(path)?;
            Ok(String::from_utf8_lossy(&data).trim().to_string())
        }
        Err(err) => Err(err),
    }
}

/// Ensures the key file is readable only by its owner (no-op on Windows).
fn restrict_to_owner(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path)?;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn unique_path() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("heigate-key-test-{pid}-{n}.key"))
    }

    #[test]
    fn creates_a_prefixed_hex_key() {
        let path = unique_path();
        let key = load_or_create_gateway_key(&path).unwrap();
        assert!(key.starts_with("hg_"), "key should carry the hg_ prefix: {key}");
        assert_eq!(key.len(), 3 + 64, "key should be hg_ plus 64 hex chars");
        assert!(key[3..].chars().all(|c| c.is_ascii_hexdigit()));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn reuses_an_existing_key() {
        let path = unique_path();
        let first = load_or_create_gateway_key(&path).unwrap();
        let second = load_or_create_gateway_key(&path).unwrap();
        assert_eq!(first, second);
        let _ = fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn restricts_existing_file_to_owner() {
        use std::os::unix::fs::PermissionsExt;
        let path = unique_path();
        fs::write(&path, "hg_existing\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let key = load_or_create_gateway_key(&path).unwrap();
        assert_eq!(key, "hg_existing");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_file(&path);
    }
}
