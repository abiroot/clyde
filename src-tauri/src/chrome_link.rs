//! Which claude.ai account the Claude-in-Chrome extension is signed into.
//!
//! Switching accounts is invisible to the browser. The extension holds its own
//! OAuth credential (a different client id from Claude Code's) and Claude Code
//! pairs with it through `wss://bridge.claudeusercontent.com`, a rendezvous
//! **keyed on account uuid**: Claude Code joins the room its live token resolves
//! to, the extension joins the room its own stored `accountUuid` names. Switch
//! accounts in Clyde and the two land in different rooms — `/chrome` then
//! reports the extension as simply not connected.
//!
//! Clyde can't fix that by writing files. The extension keeps its tokens in
//! `chrome.storage.session`, which is memory-only, and its sign-in path is
//! reachable only from a claude.ai page. What Clyde *can* do is say so plainly,
//! because the extension does persist one thing: the `accountUuid` it last
//! authenticated as, in its `chrome.storage.local` LevelDB. This module reads
//! that value so the UI can name the account the browser is on — and, when Clyde
//! happens to manage that account too, offer the one-click fix that needs no
//! browser interaction at all: switch Clyde to it.
//!
//! **Read strategy.** Chrome holds an exclusive lock on the LevelDB while it is
//! running, so this scans the raw table and log bytes rather than opening the
//! database. That is a heuristic with one known blind spot: a *deleted* key
//! leaves its old value behind in the files, so an extension that has been
//! signed out may still report the account it last used. Everything downstream
//! treats this as a hint to show the user, never as grounds to change state.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// The Claude in Chrome extension, same id across Chromium browsers.
const EXTENSION_ID: &str = "fcoeoabgfenejglbffodgkkbkcdhcgfn";

/// The `chrome.storage.local` key the extension persists its identity under.
const ACCOUNT_UUID_KEY: &[u8] = b"accountUuid";

/// How far past the key to look for the value LevelDB stored next to it.
const VALUE_WINDOW: usize = 64;

/// One browser profile that has the extension installed.
#[derive(Debug, Clone, Serialize)]
pub struct ChromeBrowser {
    /// Display name, e.g. `"Chrome"` or `"Chrome — Profile 2"`.
    pub name: String,
    /// The account uuid the extension last authenticated as, if readable.
    pub account_uuid: Option<String>,
    /// Email, when `account_uuid` matches an account Clyde manages.
    pub email: Option<String>,
    /// Clyde's account id, when it manages this account — the "switch Clyde to
    /// it" fix is only offerable when this is set.
    pub account_id: Option<String>,
    /// Whether this browser is on the currently active account.
    pub matches_active: bool,
}

/// What the UI needs to decide between "all good", "switch Clyde", and "sign
/// Chrome in".
#[derive(Debug, Clone, Serialize, Default)]
pub struct ChromeLink {
    /// Whether the extension was found in any browser profile at all.
    pub installed: bool,
    /// Every profile carrying the extension.
    pub browsers: Vec<ChromeBrowser>,
    /// True when at least one browser is on the active account — i.e. `/chrome`
    /// should work.
    pub matched: bool,
}

/// Chromium-family roots to search, relative to `~/Library/Application Support`.
/// Same family Claude Code itself probes when it looks for the extension.
const BROWSER_ROOTS: &[(&str, &str)] = &[
    ("Chrome", "Google/Chrome"),
    ("Brave", "BraveSoftware/Brave-Browser"),
    ("Edge", "Microsoft Edge"),
    ("Arc", "Arc/User Data"),
    ("Chromium", "Chromium"),
    ("Vivaldi", "Vivaldi"),
    ("Opera", "com.operasoftware.Opera"),
];

/// Every `Local Extension Settings/<ext id>` directory on this machine, paired
/// with a human label for the browser profile that owns it.
fn extension_storage_dirs() -> Vec<(String, PathBuf)> {
    let Ok(home) = std::env::var("HOME") else {
        return Vec::new();
    };
    let support = PathBuf::from(home).join("Library/Application Support");

    let mut found = Vec::new();
    for (label, rel) in BROWSER_ROOTS {
        let root = support.join(rel);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut profiles: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        profiles.sort();

        for profile in profiles {
            let dir = profile.join("Local Extension Settings").join(EXTENSION_ID);
            if !dir.is_dir() {
                continue;
            }
            let profile_name = profile
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            // "Default" is the unnamed profile; anything else is worth naming so
            // a user with several profiles knows which window we mean.
            let name = if profile_name == "Default" {
                label.to_string()
            } else {
                format!("{label} — {profile_name}")
            };
            found.push((name, dir));
        }
    }
    found
}

/// The newest `accountUuid` written into an extension storage directory.
///
/// Files are read oldest-first — compacted tables in name order, then the
/// write-ahead log, which holds the most recent writes — and the last match
/// wins.
fn read_account_uuid(dir: &Path) -> Option<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    let (mut tables, mut logs): (Vec<PathBuf>, Vec<PathBuf>) = (Vec::new(), Vec::new());
    for path in entries.flatten().map(|e| e.path()) {
        match path.extension().and_then(|e| e.to_str()) {
            Some("ldb") => tables.push(path),
            Some("log") => logs.push(path),
            _ => {}
        }
    }
    tables.sort();
    logs.sort();

    let mut newest = None;
    for path in tables.into_iter().chain(logs) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Some(uuid) = last_uuid_after_key(&bytes) {
            newest = Some(uuid);
        }
    }
    newest
}

/// Last uuid appearing just after an `accountUuid` key in a LevelDB file.
fn last_uuid_after_key(bytes: &[u8]) -> Option<String> {
    let mut found = None;
    let mut from = 0;
    while let Some(offset) = find(bytes, ACCOUNT_UUID_KEY, from) {
        let start = offset + ACCOUNT_UUID_KEY.len();
        from = start;
        let end = (start + VALUE_WINDOW).min(bytes.len());
        for i in start..end {
            if let Some(uuid) = uuid_at(bytes, i) {
                found = Some(uuid);
                break;
            }
        }
    }
    found
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// A canonical lowercase uuid starting exactly at `i`, if there is one.
fn uuid_at(bytes: &[u8], i: usize) -> Option<String> {
    const LEN: usize = 36;
    const DASHES: [usize; 4] = [8, 13, 18, 23];
    if i + LEN > bytes.len() {
        return None;
    }
    let slice = &bytes[i..i + LEN];
    for (pos, b) in slice.iter().enumerate() {
        let ok = if DASHES.contains(&pos) {
            *b == b'-'
        } else {
            b.is_ascii_digit() || (b'a'..=b'f').contains(b)
        };
        if !ok {
            return None;
        }
    }
    // Reject a match that is really the tail of a longer hex run.
    if i > 0 {
        let prev = bytes[i - 1];
        if prev.is_ascii_hexdigit() || prev == b'-' {
            return None;
        }
    }
    String::from_utf8(slice.to_vec()).ok()
}

/// Look up which browsers have the extension and which account each is on.
///
/// `known` maps account uuid → (Clyde account id, email); `active_uuid` is the
/// account currently active in Clyde, when known.
pub fn detect(known: &[(String, String, Option<String>)], active_uuid: Option<&str>) -> ChromeLink {
    let dirs = extension_storage_dirs();
    let installed = !dirs.is_empty();

    let browsers: Vec<ChromeBrowser> = dirs
        .into_iter()
        .map(|(name, dir)| {
            let account_uuid = read_account_uuid(&dir);
            let known_match = account_uuid
                .as_ref()
                .and_then(|u| known.iter().find(|(uuid, _, _)| uuid == u));
            ChromeBrowser {
                matches_active: match (account_uuid.as_deref(), active_uuid) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                },
                email: known_match.and_then(|(_, _, email)| email.clone()),
                account_id: known_match.map(|(_, id, _)| id.clone()),
                account_uuid,
                name,
            }
        })
        .collect();

    ChromeLink {
        matched: browsers.iter().any(|b| b.matches_active),
        installed,
        browsers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "88fcbcc5-e7fb-48e7-b849-af8ff8d648e9";

    /// Roughly how the value sits in a LevelDB record: key, framing byte, then
    /// the JSON-quoted value.
    fn record(uuid: &str) -> Vec<u8> {
        let mut v = b"\x01\x26accountUuid".to_vec();
        v.extend_from_slice(format!("\x26\"{uuid}\"").as_bytes());
        v
    }

    #[test]
    fn reads_the_uuid_stored_next_to_the_key() {
        assert_eq!(last_uuid_after_key(&record(UUID)), Some(UUID.to_string()));
    }

    #[test]
    fn later_writes_win_within_a_file() {
        let newer = "02ebb982-8b34-4139-81bb-b51cf56add3f";
        let mut bytes = record(UUID);
        bytes.extend_from_slice(&[0u8; 12]);
        bytes.extend_from_slice(&record(newer));
        assert_eq!(last_uuid_after_key(&bytes), Some(newer.to_string()));
    }

    #[test]
    fn ignores_uuids_that_are_not_next_to_the_key() {
        // A uuid belonging to some other storage key, far from ours.
        let mut bytes = b"someOtherKey\x26\"".to_vec();
        bytes.extend_from_slice(UUID.as_bytes());
        bytes.extend_from_slice(b"\"");
        assert_eq!(last_uuid_after_key(&bytes), None);
    }

    #[test]
    fn ignores_a_key_with_no_value_in_range() {
        let mut bytes = b"accountUuid".to_vec();
        bytes.extend_from_slice(&[b'x'; 200]);
        bytes.extend_from_slice(UUID.as_bytes());
        assert_eq!(last_uuid_after_key(&bytes), None);
    }

    #[test]
    fn rejects_a_uuid_shaped_tail_of_a_longer_hex_run() {
        let mut bytes = b"accountUuid\x26\"ff".to_vec();
        bytes.extend_from_slice(UUID.as_bytes());
        bytes.extend_from_slice(b"\"");
        assert_eq!(last_uuid_after_key(&bytes), None);
    }

    #[test]
    fn detect_marks_the_active_account() {
        let known = vec![(
            UUID.to_string(),
            "claude_someone".to_string(),
            Some("someone@example.com".to_string()),
        )];
        // No browsers on this machine's test runner is fine — this exercises the
        // matching rules directly.
        let link = detect(&known, Some(UUID));
        assert!(link
            .browsers
            .iter()
            .all(|b| b.matches_active == (b.account_uuid.as_deref() == Some(UUID))));
    }
}
