//! The local gateway credential. It lives outside the database and is
//! restricted to the current user, matching the Go `internal/api` auth code.

use std::fs;
use std::io::{self, Read, Seek, Write};
use std::path::Path;

/// Prefix of every generated gateway key.
const KEY_PREFIX: &str = "hg_";

/// Reads the gateway key from `path`, creating it (0600) with a fresh random
/// value when the file is missing or empty.
pub fn load_or_create_gateway_key(path: &Path) -> io::Result<String> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // Serialize both creation and empty-file repair across processes; nobody
    // can adopt a partially written key from another startup.
    file.lock()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let mut data = String::new();
    file.read_to_string(&mut data)?;
    let existing = data.trim();
    if !existing.is_empty() {
        return Ok(existing.to_string());
    }
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).map_err(|err| io::Error::other(err.to_string()))?;
    let key = format!("{KEY_PREFIX}{}", hex_encode(&buf));
    file.rewind()?;
    file.set_len(0)?;
    writeln!(file, "{key}")?;
    file.sync_all()?;
    Ok(key)
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
        assert!(
            key.starts_with("hg_"),
            "key should carry the hg_ prefix: {key}"
        );
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

    #[test]
    fn empty_key_repair_is_secure_and_shared_by_concurrent_startups() {
        for exists in [false, true] {
            let path = unique_path();
            if exists {
                fs::write(&path, " \n").unwrap();
            }
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let threads = (0..8)
                .map(|_| {
                    let path = path.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        load_or_create_gateway_key(&path).unwrap()
                    })
                })
                .collect::<Vec<_>>();
            let keys = threads
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(keys[0].len(), 67);
            assert!(keys.iter().all(|k| k == &keys[0]));
            assert_eq!(fs::read_to_string(&path).unwrap().trim(), keys[0]);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            fs::remove_file(path).unwrap();
        }
    }
}
