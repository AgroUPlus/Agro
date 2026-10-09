//! The two-factor key, when the operator did not supply one.
//!
//! `AGRO_SECRET_KEY` is the explicit way and always wins. Without it, requiring someone to run
//! `openssl` before they can use a feature the dashboard recommends is a poor first run, so the
//! server mints a random key once and keeps it in `agro_secret.key` beside the database.
//!
//! **What that does and does not protect.** The key exists so two-factor secrets are encrypted in
//! the database: a copy of `agro_data.db` alone (an off-box backup, a leaked dump) cannot open
//! them. A key file on the same disk gives up the case where the whole data directory is taken.
//! Operators who want the key held elsewhere set `AGRO_SECRET_KEY` and it is used instead.
//!
//! The file is created `0600`, never overwritten, and never replaced by a new key when it is
//! unreadable or too short — a silently regenerated key would orphan every enrolled secret.

use std::io::{self, Write};
use std::path::Path;
use std::sync::OnceLock;

/// File name, resolved against the working directory like the database.
pub const FILE_NAME: &str = "agro_secret.key";
const MIN_LEN: usize = 16;

static LOADED: OnceLock<String> = OnceLock::new();

/// The key loaded at boot from the file, if the environment did not provide one.
pub fn loaded() -> Option<&'static str> {
    LOADED.get().map(String::as_str)
}

/// Reads the key file, creating it with a fresh random key if it does not exist, and makes the key
/// available to [`loaded`]. Returns whether a new key was generated.
pub fn provide(path: &Path) -> io::Result<bool> {
    let (key, created) = load_or_create(path)?;
    let _ = LOADED.set(key);
    Ok(created)
}

fn load_or_create(path: &Path) -> io::Result<(String, bool)> {
    match std::fs::read_to_string(path) {
        Ok(existing) => {
            let key = existing.trim().to_string();
            if key.len() < MIN_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "{} exists but is not a usable key; refusing to replace it",
                        path.display()
                    ),
                ));
            }
            Ok((key, false))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let key = crate::credentials::mint_token().secret;
            write_new(path, &key)?;
            Ok((key, true))
        }
        Err(err) => Err(err),
    }
}

fn write_new(path: &Path, key: &str) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agro-key-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(FILE_NAME)
    }

    #[test]
    fn a_key_is_made_once_and_then_reused() {
        let path = scratch("reuse");
        let _ = std::fs::remove_file(&path);
        let (first, created) = load_or_create(&path).unwrap();
        assert!(created && first.len() >= MIN_LEN);
        let (second, created_again) = load_or_create(&path).unwrap();
        assert!(!created_again);
        assert_eq!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("mode");
        let _ = std::fs::remove_file(&path);
        load_or_create(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn an_unusable_file_is_refused_not_replaced() {
        let path = scratch("short");
        std::fs::write(&path, "short\n").unwrap();
        assert!(load_or_create(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "short\n");
    }
}
