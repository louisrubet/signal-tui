//! Encryption of the store at rest (SQLCipher), and where its key lives.
//!
//! The key is a random secret kept in the OS keyring (Secret Service on Linux, Keychain on
//! macOS, Credential Manager on Windows). Without a reachable keyring (headless Linux, SSH),
//! it is a passphrase typed at startup, or given in `$SIGNAL_TUI_PASSPHRASE`.

use std::error::Error;
use std::str::FromStr;

use rand::RngCore;
use sqlx::ConnectOptions;
use sqlx::sqlite::SqliteConnectOptions;

const KEYRING_SERVICE: &str = "signal-tui";
const PASSPHRASE_VAR: &str = "SIGNAL_TUI_PASSPHRASE";

/// What is on disk at the store path.
#[derive(Debug, PartialEq)]
pub enum StoreState {
    Missing,
    /// A SQLite file in clear, as made by earlier versions.
    Plain,
    Encrypted,
}

pub fn store_state(path: &str) -> std::io::Result<StoreState> {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StoreState::Missing),
        Err(e) => Err(e),
        Ok(bytes) if bytes.is_empty() => Ok(StoreState::Missing),
        Ok(bytes) if bytes.starts_with(b"SQLite format 3\0") => Ok(StoreState::Plain),
        Ok(_) => Ok(StoreState::Encrypted),
    }
}

/// The key of the store at `path`: `$SIGNAL_TUI_PASSPHRASE`, else the keyring entry (made
/// with a new random key if there is none yet), else a passphrase asked on the terminal.
pub fn store_key(path: &str) -> Result<String, Box<dyn Error>> {
    if let Ok(passphrase) = std::env::var(PASSPHRASE_VAR) {
        return Ok(passphrase);
    }
    let state = store_state(path)?;
    match keyring::Entry::new(KEYRING_SERVICE, path) {
        Ok(entry) => match entry.get_password() {
            Ok(key) => return Ok(key),
            // An encrypted store without keyring entry was made with a passphrase.
            Err(keyring::Error::NoEntry) if state == StoreState::Encrypted => {}
            Err(keyring::Error::NoEntry) => {
                let key = random_key();
                entry.set_password(&key)?;
                return Ok(key);
            }
            Err(e) => eprintln!("Keyring unavailable ({e}), using a passphrase."),
        },
        Err(e) => eprintln!("Keyring unavailable ({e}), using a passphrase."),
    }
    ask_passphrase(state == StoreState::Encrypted)
}

/// Opens the store at `path` encrypted: gets its key, and first encrypts a plain store left
/// by an earlier version.
pub async fn open_encrypted(path: &str) -> Result<presage_store_sqlite::SqliteStore, Box<dyn Error>> {
    restrict_new_files();
    let key = store_key(path)?;
    if store_state(path)? == StoreState::Plain {
        encrypt_store(path, &key).await?;
        eprintln!("Store {path} encrypted.");
    }
    crate::signal::open_store(path, Some(&key))
        .await
        .map_err(|e| format!("cannot open the store {path} (wrong passphrase?): {e}").into())
}

/// New files (store, its WAL, read state…) readable by us only.
fn restrict_new_files() {
    #[cfg(unix)]
    // SAFETY: umask only sets the process file mode creation mask.
    unsafe {
        libc::umask(0o077);
    }
}

/// Removes the keyring entry of the store at `path`, if any.
pub fn forget_store_key(path: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, path) {
        let _ = entry.delete_credential();
    }
}

fn random_key() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn ask_passphrase(existing: bool) -> Result<String, Box<dyn Error>> {
    if existing {
        return Ok(rpassword::prompt_password("Store passphrase: ")?);
    }
    loop {
        let passphrase = rpassword::prompt_password("Choose a passphrase for the store: ")?;
        if passphrase.is_empty() {
            eprintln!("The passphrase cannot be empty.");
            continue;
        }
        if rpassword::prompt_password("Same passphrase again: ")? == passphrase {
            return Ok(passphrase);
        }
        eprintln!("The passphrases differ, try again.");
    }
}

/// Encrypts the plain store at `path` with `key`, in place (SQLCipher export).
pub async fn encrypt_store(path: &str, key: &str) -> Result<(), Box<dyn Error>> {
    let encrypted = format!("{path}.encrypting");
    let _ = std::fs::remove_file(&encrypted);

    // Creating files is needed for the attached encrypted one.
    let mut conn = SqliteConnectOptions::from_str(path)?.create_if_missing(true).connect().await?;
    // The export reads through this connection, so it includes what is still in the WAL.
    sqlx::query("ATTACH DATABASE ?1 AS encrypted KEY ?2").bind(&encrypted).bind(key).execute(&mut conn).await?;
    sqlx::query("SELECT sqlcipher_export('encrypted')").execute(&mut conn).await?;
    sqlx::query("DETACH DATABASE encrypted").execute(&mut conn).await?;
    use sqlx::Connection;
    conn.close().await?;

    // The plain WAL must not be applied to the encrypted file: remove it first.
    for leftover in [format!("{path}-wal"), format!("{path}-shm")] {
        match std::fs::remove_file(&leftover) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    std::fs::rename(&encrypted, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal;

    #[tokio::test]
    async fn plain_store_is_encrypted_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db3");
        let path = path.to_str().unwrap();

        // A plain store, as made by earlier versions.
        drop(signal::open_store(path, None).await.unwrap());
        assert_eq!(store_state(path).unwrap(), StoreState::Plain);

        encrypt_store(path, "secret").await.unwrap();
        assert_eq!(store_state(path).unwrap(), StoreState::Encrypted);
        assert!(!std::fs::read(path).unwrap().windows(9).any(|w| w == b"_sqlx_mig"), "no clear text left");

        let store = signal::open_store(path, Some("secret")).await.unwrap();
        assert!(matches!(signal::load_registered(store).await, Err(presage::Error::NotYetRegisteredError)));
        assert!(signal::open_store(path, Some("wrong")).await.is_err(), "wrong key");
    }

    #[test]
    fn missing_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("none.db3");
        assert_eq!(store_state(path.to_str().unwrap()).unwrap(), StoreState::Missing);
    }
}
