//! Encrypted-at-rest storage for accounts.
//!
//! All credentials live in the OS-native secret store via the `keyring` crate:
//! macOS Keychain, Windows Credential Manager, or the Linux Secret Service.
//! The whole account list is serialized to a single JSON blob and stored under
//! one keychain entry — Clyde never writes tokens to plaintext files.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::model::Account;
use anyhow::{bail, Context, Result};
use keyring::Entry;

/// Set once the stored list has been read. Until then saving is refused: a
/// list that failed to load is not an empty list, and writing one would wipe
/// every saved account.
static LOADED: AtomicBool = AtomicBool::new(false);

const SERVICE: &str = "com.clyde.app";
const ACCOUNT_KEY: &str = "accounts-v1";

fn entry() -> Result<Entry> {
    Entry::new(SERVICE, ACCOUNT_KEY).context("opening keychain entry")
}

/// Load all stored accounts. Returns an empty list on first run.
pub fn load_accounts() -> Result<Vec<Account>> {
    // Debug builds only: a test copy of Clyde (see `provider::test_keys`)
    // starts with no Claude accounts and never saves any, so it can't touch
    // the real list.
    #[cfg(debug_assertions)]
    if std::env::var_os("CLYDE_TEST_KEYS").is_some() {
        return Ok(Vec::new());
    }
    let entry = entry()?;
    match entry.get_password() {
        Ok(json) => {
            let accounts: Vec<Account> =
                serde_json::from_str(&json).context("deserializing accounts from keychain")?;
            LOADED.store(true, Ordering::SeqCst);
            Ok(accounts)
        }
        Err(keyring::Error::NoEntry) => {
            LOADED.store(true, Ordering::SeqCst);
            Ok(Vec::new())
        }
        Err(e) => Err(e).context("reading accounts from keychain"),
    }
}

/// Persist the full account list, replacing whatever was there.
pub fn save_accounts(accounts: &[Account]) -> Result<()> {
    if !LOADED.load(Ordering::SeqCst) {
        bail!("the saved accounts haven't been read from the Keychain yet; not overwriting them");
    }
    let json = serde_json::to_string(accounts).context("serializing accounts")?;
    entry()?
        .set_password(&json)
        .context("writing accounts to keychain")
}

/// Whether the stored list has been read (see [`LOADED`]).
pub fn is_loaded() -> bool {
    LOADED.load(Ordering::SeqCst)
}

/// Remove the keychain entry entirely (used by "reset / sign out of everything").
#[allow(dead_code)] // exposed for an upcoming "reset" command
pub fn wipe() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("deleting keychain entry"),
    }
}
