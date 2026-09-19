//! Make a Clyde account the one Claude Code uses, by writing its OAuth directly
//! into Claude Code's own credential store — no proxy, no `settings.json` edits.
//!
//! Claude Code (default config dir) reads its subscription token from the macOS
//! Keychain item `Claude Code-credentials`, and shows identity from its global
//! config file → `oauthAccount` (see [`claude_json_path`] for which file that
//! is). To switch accounts we rewrite both, in place, preserving everything
//! else. Plain `claude` then talks straight to api.anthropic.com as the chosen
//! account — whether or not Clyde is running.
//!
//! Clyde always targets the user's *default* `claude` (the `~/.claude` config
//! dir / unsuffixed keychain service), regardless of any `CLAUDE_CONFIG_DIR` the
//! Clyde process itself may have inherited.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};

use crate::model::{Account, Credential};

/// Keychain service for the default config dir.
const SERVICE: &str = "Claude Code-credentials";

/// The throwaway `apiKeyHelper` value the old proxy integration injected.
const LEGACY_HELPER_MARKER: &str = "echo clyde-managed-token";

fn home() -> Result<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| anyhow!("no home directory"))
}

fn claude_dir() -> Result<PathBuf> {
    Ok(home()?.join(".claude"))
}

/// The global config file Claude Code actually reads, mirroring its own
/// resolution order (verified against Claude Code 2.1.227): `~/.claude/.config.json`
/// when that file exists, otherwise `~/.claude.json`.
///
/// Claude Code also honours `CLAUDE_CONFIG_DIR` at this step; Clyde deliberately
/// does not, for the same reason it pins the unsuffixed keychain service — the
/// job is to drive the `claude` the *user* runs, not whatever config dir Clyde's
/// own process happened to inherit. See [`legacy_claude_json_path`] for the file
/// earlier versions wrote instead.
fn claude_json_path() -> Result<PathBuf> {
    let dot_config = claude_dir()?.join(".config.json");
    if dot_config.exists() {
        return Ok(dot_config);
    }
    Ok(home()?.join(".claude.json"))
}

/// `~/.claude/.claude.json` — the file Clyde wrote before it mirrored Claude
/// Code's real resolution order, and still the correct target for anyone whose
/// `CLAUDE_CONFIG_DIR` is `~/.claude`. Kept in sync whenever it already exists,
/// so the two files can never disagree about who is logged in.
fn legacy_claude_json_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join(".claude.json"))
}

fn settings_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join("settings.json"))
}

// ---- keychain -------------------------------------------------------------

/// Read the raw `Claude Code-credentials` secret, if present.
fn read_secret() -> Option<String> {
    read_secret_strict().ok().flatten()
}

/// `security`'s exit status for errSecItemNotFound.
const SECURITY_ITEM_NOT_FOUND: i32 = 44;

/// Like [`read_secret`], but tells "no item yet" (`Ok(None)`) apart from "the
/// item exists and couldn't be read" (`Err`) — a locked keychain, a denied
/// prompt. Read-modify-write callers must not treat the second as empty.
fn read_secret_strict() -> Result<Option<String>> {
    read_secret_from(SERVICE)
}

fn read_secret_from(service: &str) -> Result<Option<String>> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .context("running `security find-generic-password`")?;
    if out.status.code() == Some(SECURITY_ITEM_NOT_FOUND) {
        return Ok(None);
    }
    if !out.status.success() {
        return Err(anyhow!(
            "couldn't read the Claude Code keychain item: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let s = String::from_utf8(out.stdout)
        .context("keychain item isn't UTF-8")?
        .trim()
        .to_string();
    Ok((!s.is_empty()).then(|| decode_if_hex(s)))
}

/// `security -w` prints a password that isn't plain ASCII (e.g. an org name
/// with an accent) as bare hex. A JSON blob never starts with a hex digit, so
/// all-hex output is that encoding — decode it back.
fn decode_if_hex(s: String) -> String {
    let is_hex = s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit());
    if !is_hex {
        return s;
    }
    let bytes: Option<Vec<u8>> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect();
    bytes.and_then(|b| String::from_utf8(b).ok()).unwrap_or(s)
}

/// Read the `acct` attribute of the existing item so an in-place update matches
/// it exactly (and doesn't create a duplicate item). Empty string if unknown.
fn read_account_attr() -> String {
    let Ok(out) = Command::new("security")
        .args(["find-generic-password", "-s", SERVICE, "-g"])
        .output()
    else {
        return String::new();
    };
    // `-g` prints the human-readable attribute dump to stderr.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("\"acct\"<blob>=") {
            if let Some(inner) = rest
                .trim()
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
            {
                return inner.to_string();
            }
        }
    }
    String::new()
}

/// Write the secret back into the existing item in place. `-U` updates rather
/// than duplicates, preserving the item's existing access-control list so plain
/// `claude` keeps reading it without a prompt.
///
/// Mirrors Claude Code's own writer (2.1.278): the secret goes hex-encoded
/// (`-X`) through `security -i` so it stays out of the process argument list —
/// unless the command line is too long for `security -i`, in which case it falls
/// back to argv exactly as Claude Code does. The limit is real and silent:
/// `security -i` stores the truncated head of an over-long line as the password
/// and only then fails, and `mcpOAuth` alone pushes this item past it.
fn write_secret(secret: &str, acct: &str) -> Result<()> {
    write_secret_to(SERVICE, secret, acct)
}

/// Longest `security -i` line that is stored intact, with margin. Measured on
/// macOS 15: a 4,200-byte line kept 4,038 bytes of password, then exited 1.
const SECURITY_STDIN_LINE_MAX: usize = 4000;

/// How [`write_secret_to`] hands the secret to `security`.
#[derive(Debug, PartialEq)]
enum SecretWrite {
    /// One `security -i` line (keeps the secret out of `ps`).
    Stdin(String),
    /// Plain argv — only when the stdin line would be truncated.
    Argv(Vec<String>),
}

fn secret_write_command(service: &str, secret: &str, acct: &str) -> SecretWrite {
    let hex: String = secret.bytes().map(|b| format!("{b:02x}")).collect();

    // security(1)'s stdin parser supports double-quoted words with backslash
    // escapes; the hex payload itself needs none.
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut line = format!("add-generic-password -U -s \"{}\"", esc(service));
    if !acct.is_empty() {
        line.push_str(&format!(" -a \"{}\"", esc(acct)));
    }
    line.push_str(&format!(" -X \"{hex}\"\n"));
    if line.len() <= SECURITY_STDIN_LINE_MAX {
        return SecretWrite::Stdin(line);
    }

    let mut args: Vec<String> = vec!["add-generic-password".into(), "-U".into()];
    args.extend(["-s".into(), service.into()]);
    if !acct.is_empty() {
        args.extend(["-a".into(), acct.into()]);
    }
    args.extend(["-X".into(), hex]);
    SecretWrite::Argv(args)
}

fn write_secret_to(service: &str, secret: &str, acct: &str) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let out = match secret_write_command(service, secret, acct) {
        SecretWrite::Stdin(line) => {
            let mut child = Command::new("security")
                .arg("-i")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("running `security -i`")?;
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("no stdin handle for `security -i`"))?
                .write_all(line.as_bytes())
                .context("writing to `security -i`")?;
            child
                .wait_with_output()
                .context("waiting for `security -i`")?
        }
        SecretWrite::Argv(args) => Command::new("security")
            .args(&args)
            .output()
            .context("running `security add-generic-password`")?,
    };
    if !out.status.success() {
        return Err(anyhow!(
            "security add-generic-password failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

// ---- public API -----------------------------------------------------------

/// Rewrite the identity block if it doesn't already describe `account`.
///
/// The keychain slot is the truth about who is logged in; the config file is
/// only a label. They drift apart in two ways worth healing: a Clyde old enough
/// to have written the wrong file left a stale identity behind, and `claude`
/// itself may have recorded a different account before Clyde took over the slot.
/// Either way Claude Code goes on comparing that stale `accountUuid` against the
/// account its live token resolves to — which is what surfaces as "belongs to a
/// different claude.ai account" on the Chrome bridge.
///
/// Returns whether anything was written.
pub fn repair_identity(account: &Account) -> Result<bool> {
    let want_uuid = account
        .oauth_account
        .as_ref()
        .and_then(|v| v.get("accountUuid"))
        .and_then(|v| v.as_str());

    let email_matches = account.email.is_none() || read_active_identity_email() == account.email;
    let uuid_matches = match want_uuid {
        Some(want) => read_active_identity_uuid().as_deref() == Some(want),
        None => true,
    };
    if email_matches && uuid_matches {
        return Ok(false);
    }
    update_claude_json(account)?;
    Ok(true)
}

/// Make `account` the active Claude Code account: write its OAuth into the
/// keychain and update `.claude.json`'s identity to match.
pub fn activate(account: &Account) -> Result<()> {
    // Preserve any other top-level keys already in the blob (e.g. `mcpOAuth`).
    // A read failure (locked keychain, denied prompt) aborts: starting empty
    // would erase every MCP server's login. Keychain reads are never torn, so an
    // unparsable item is already corrupt — e.g. truncated by an old Clyde — and
    // `claude` can't use it either; overwriting it is the repair.
    let mut root: Map<String, Value> = match read_secret_strict()? {
        None => Map::new(),
        Some(s) => match serde_json::from_str(&s) {
            Ok(Value::Object(m)) => m,
            _ => {
                tracing::warn!("Claude Code's keychain item was corrupt; rewriting it");
                Map::new()
            }
        },
    };

    // Plan metadata from the *outgoing* blob, used only as a fallback when the
    // incoming account didn't capture its own (e.g. added via token paste).
    let prev = root
        .get("claudeAiOauth")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let mut oauth = Map::new();
    oauth.insert("accessToken".into(), json!(account.credential.access_token));
    oauth.insert(
        "refreshToken".into(),
        json!(account.credential.refresh_token),
    );
    oauth.insert("expiresAt".into(), json!(account.credential.expires_at));
    // Never write an empty scope list — Claude Code would see a scopeless
    // session. Fall back to whatever the blob already carried.
    let scopes = if account.credential.scopes.is_empty() {
        prev.get("scopes").cloned().unwrap_or_else(|| json!([]))
    } else {
        json!(account.credential.scopes)
    };
    oauth.insert("scopes".into(), scopes);

    // Write *this* account's plan, not whatever the previous account left behind.
    let subscription = account.subscription_raw.clone().or_else(|| {
        prev.get("subscriptionType")
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    if let Some(sub) = &subscription {
        oauth.insert("subscriptionType".into(), json!(sub));
    }
    if let Some(tier) = account.rate_limit_tier.clone().or_else(|| {
        prev.get("rateLimitTier")
            .and_then(|v| v.as_str())
            .map(String::from)
    }) {
        oauth.insert("rateLimitTier".into(), json!(tier));
    }
    if let Some(is_max) = subscription
        .as_deref()
        .map(|s| s == "max")
        .or_else(|| prev.get("isMax").and_then(|v| v.as_bool()))
    {
        oauth.insert("isMax".into(), json!(is_max));
    }
    root.insert("claudeAiOauth".into(), Value::Object(oauth));

    let secret = serde_json::to_string(&Value::Object(root))?;
    write_secret(&secret, &read_account_attr())?;

    update_claude_json(account)?;
    Ok(())
}

/// Read the credential Claude Code currently holds for the default account, so
/// Clyde can keep its own copy of the active account in sync with the rotations
/// Claude Code performs independently.
pub fn read_active_credential() -> Option<Credential> {
    let secret = read_secret()?;
    let v: Value = serde_json::from_str(&secret).ok()?;
    let o = v.get("claudeAiOauth")?;
    Some(Credential {
        access_token: o.get("accessToken")?.as_str()?.to_string(),
        refresh_token: o
            .get("refreshToken")
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string(),
        expires_at: o.get("expiresAt").and_then(|x| x.as_i64()).unwrap_or(0),
        scopes: o
            .get("scopes")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// Push a rotated credential for the currently active account back into Claude
/// Code's keychain slot. Without this, a refresh Clyde performs (e.g. for usage
/// polling) consumes the refresh token while the keychain keeps the dead old
/// one — and the next `claude` run gets force-logged-out.
///
/// Guarded: only writes when the slot still holds the generation we rotated
/// from (`expected_old`), so a login the user did via `claude /login` in the
/// meantime is never clobbered. Returns whether the slot was updated.
pub fn write_active_credential(new: &Credential, expected_old: &Credential) -> Result<bool> {
    let Some(secret) = read_secret() else {
        return Ok(false);
    };
    let Ok(Value::Object(mut root)) = serde_json::from_str(&secret) else {
        return Ok(false);
    };
    let held_matches = root.get("claudeAiOauth").is_some_and(|o| {
        let tok = |k: &str| o.get(k).and_then(|v| v.as_str()).unwrap_or_default();
        let same_old = (!expected_old.refresh_token.is_empty()
            && tok("refreshToken") == expected_old.refresh_token)
            || tok("accessToken") == expected_old.access_token;
        let already_new = tok("accessToken") == new.access_token;
        same_old || already_new
    });
    if !held_matches {
        return Ok(false);
    }
    merge_credential(&mut root, new);
    let secret = serde_json::to_string(&Value::Object(root))?;
    write_secret(&secret, &read_account_attr())?;
    Ok(true)
}

/// Update just the token fields of a credential blob, leaving identity/plan
/// keys and sibling top-level keys (e.g. `mcpOAuth`) intact. An empty `scopes`
/// keeps the blob's existing list — some refresh responses omit scopes, and
/// writing `[]` would make Claude Code see a scopeless session.
fn merge_credential(root: &mut Map<String, Value>, cred: &Credential) {
    let oauth = root
        .entry("claudeAiOauth")
        .or_insert_with(|| Value::Object(Map::new()));
    if !oauth.is_object() {
        *oauth = Value::Object(Map::new());
    }
    let o = oauth.as_object_mut().expect("ensured object above");
    o.insert("accessToken".into(), json!(cred.access_token));
    o.insert("refreshToken".into(), json!(cred.refresh_token));
    o.insert("expiresAt".into(), json!(cred.expires_at));
    if !cred.scopes.is_empty() {
        o.insert("scopes".into(), json!(cred.scopes));
    } else if !o.contains_key("scopes") {
        o.insert("scopes".into(), json!([]));
    }
}

/// Count running `claude` processes (exact process name). Used to warn right
/// after a switch: live sessions adopt the new credential within ~30 s (Claude
/// Code's keychain reads are cached for 30 s, not "until restart"), and a
/// session's Claude-in-Chrome browser bridge is account-bound — it drops on the
/// account change and only recovers via `/chrome` → "Reconnect extension" or a
/// session restart.
pub fn running_claude_sessions() -> u32 {
    let Ok(out) = Command::new("pgrep").args(["-x", "claude"]).output() else {
        return 0;
    };
    if !out.status.success() {
        return 0; // no matches (exit 1) or pgrep unavailable
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count() as u32
}

/// The email of the identity Claude Code currently displays, from its global
/// config → `oauthAccount.emailAddress`. Tokens are opaque, so this is the only
/// offline way to tell *whose* credential the shared keychain slot holds.
///
/// Falls back to the legacy path so an upgrade doesn't briefly lose track of the
/// active identity before the next switch rewrites the real file.
pub fn read_active_identity_email() -> Option<String> {
    identity_field("emailAddress")
}

/// The `accountUuid` Claude Code has on file for the active identity — the same
/// value the Chrome extension keys its bridge connection on.
pub fn read_active_identity_uuid() -> Option<String> {
    identity_field("accountUuid")
}

fn identity_field(key: &str) -> Option<String> {
    let paths = [claude_json_path().ok(), legacy_claude_json_path().ok()];
    for path in paths.into_iter().flatten() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(found) = v
            .get("oauthAccount")
            .and_then(|o| o.get(key))
            .and_then(|x| x.as_str())
        {
            return Some(found.to_string());
        }
    }
    None
}

/// One-time self-heal: strip a stale proxy integration left in `settings.json`
/// by an older Clyde (so upgrading users aren't stuck pointing `claude` at a
/// dead proxy). Returns whether anything was removed.
pub fn cleanup_legacy_integration() -> Result<bool> {
    cleanup_legacy_at(
        &settings_path()?,
        &claude_dir()?.join(".clyde-integration-backup.json"),
    )
}

/// Strip Clyde's own proxy keys from a settings file, leaving everything else
/// (including a user's *own* `apiKeyHelper` / base URL) untouched.
fn cleanup_legacy_at(path: &Path, backup: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(false);
    }
    let mut root: Map<String, Value> = match serde_json::from_str(&text) {
        Ok(Value::Object(m)) => m,
        _ => return Ok(false),
    };
    let mut changed = false;

    if root.get("apiKeyHelper").and_then(|v| v.as_str()) == Some(LEGACY_HELPER_MARKER) {
        root.remove("apiKeyHelper");
        changed = true;
    }
    if let Some(env) = root.get_mut("env").and_then(|e| e.as_object_mut()) {
        let is_clyde_proxy = env
            .get("ANTHROPIC_BASE_URL")
            .and_then(|v| v.as_str())
            .is_some_and(|u| u.contains("127.0.0.1") || u.contains("localhost"));
        if is_clyde_proxy {
            env.remove("ANTHROPIC_BASE_URL");
            changed = true;
        }
        if env.is_empty() {
            root.remove("env");
        }
    }

    if changed {
        write_json_atomic(path, &root)?;
        let _ = std::fs::remove_file(backup);
    }
    Ok(changed)
}

// ---- .claude.json ---------------------------------------------------------

/// Claude Code's global config, parsed — for reading keys Clyde doesn't own
/// (e.g. `mcpServers`). `None` when missing or unreadable.
pub fn read_global_config() -> Option<Value> {
    let text = std::fs::read_to_string(claude_json_path().ok()?).ok()?;
    serde_json::from_str(&text).ok()
}

/// Set one boolean top-level key in the global config, preserving everything
/// else — same file resolution and legacy mirroring as the identity write.
pub fn set_global_flag(key: &str, value: bool) -> Result<()> {
    let set = |path: &Path| -> Result<()> {
        let mut root = read_json_object(path)?;
        root.insert(key.into(), json!(value));
        write_json_atomic(path, &root)
    };
    let main = claude_json_path()?;
    set(&main)?;
    let legacy = legacy_claude_json_path()?;
    if legacy.exists() && legacy != main {
        set(&legacy)?;
    }
    Ok(())
}

fn update_claude_json(account: &Account) -> Result<()> {
    write_identity(&claude_json_path()?, account)?;
    // Only ever *update* the legacy file — never create one, or we'd resurrect
    // the very split-brain this fix removes.
    let legacy = legacy_claude_json_path()?;
    if legacy.exists() && Some(&legacy) != claude_json_path().ok().as_ref() {
        write_identity(&legacy, account)?;
    }
    Ok(())
}

fn write_identity(path: &Path, account: &Account) -> Result<()> {
    let mut root = read_json_object(path)?;

    let oauth_account = if let Some(meta) = &account.oauth_account {
        // Replace wholesale rather than merge: the fields we'd leave behind
        // (organizationName, seatTier, …) describe the *outgoing* account, and
        // showing another account's org is worse than showing none.
        meta.clone()
    } else {
        // No captured identity: patch the email onto whatever's there.
        let mut existing = root
            .get("oauthAccount")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let (Some(obj), Some(email)) = (existing.as_object_mut(), account.email.as_ref()) {
            obj.insert("emailAddress".into(), json!(email));
        }
        existing
    };
    root.insert("oauthAccount".into(), oauth_account);
    write_json_atomic(path, &root)
}

/// Read a JSON-object config file for a read-modify-write. A missing file is an
/// empty object; anything else that fails to parse is an error, never a blank
/// slate — writing a blank slate back is how a 400 KB `.claude.json` becomes
/// `{"oauthAccount": …}`. `claude` rewrites these files constantly, so a parse
/// failure is retried briefly in case we caught one mid-write.
fn read_json_object(path: &Path) -> Result<Map<String, Value>> {
    let name = path.display();
    let mut last_err = None;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {name}")),
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(m)) => return Ok(m),
            Ok(_) => return Err(anyhow!("{name} isn't a JSON object; leaving it alone")),
            Err(e) => last_err = Some(e),
        }
    }
    Err(anyhow!(
        "{name} isn't valid JSON ({}); leaving it alone",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// Replace a config file atomically: write a sibling temp file, then rename it
/// over the target, so neither a crash nor a concurrent reader ever sees a
/// half-written file. Symlinks are followed (the link survives) and an existing
/// file's permissions are kept; new files are owner-only.
fn write_json_atomic(path: &Path, root: &Map<String, Value>) -> Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", target.display()))?;
    let file_name = target
        .file_name()
        .ok_or_else(|| anyhow!("{} has no file name", target.display()))?
        .to_string_lossy();
    let tmp = dir.join(format!(".{file_name}.clyde-{}.tmp", std::process::id()));

    let body = serde_json::to_string_pretty(&Value::Object(root.clone()))?;
    let result = (|| -> Result<()> {
        std::fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        match std::fs::metadata(&target) {
            Ok(meta) => std::fs::set_permissions(&tmp, meta.permissions())?,
            Err(_) => set_private(&tmp),
        }
        std::fs::rename(&tmp, &target).with_context(|| format!("replacing {}", target.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
fn set_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("clyde_test_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn account_with_email(email: &str) -> Account {
        serde_json::from_value(json!({
            "id": "t", "label": "t", "email": email,
            "credential": { "access_token": "a", "refresh_token": "r", "expires_at": 0, "scopes": [] }
        }))
        .unwrap()
    }

    #[test]
    fn identity_write_refuses_an_unparsable_config_and_leaves_it_untouched() {
        let path = tmp("torn").join(".claude.json");
        let torn = r#"{"projects": {"/a": {}}, "mcpServers": {"x"#; // caught mid-write
        std::fs::write(&path, torn).unwrap();

        assert!(write_identity(&path, &account_with_email("b@x.com")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), torn);
    }

    #[test]
    fn identity_write_keeps_every_other_key_and_follows_symlinks() {
        let dir = tmp("keep");
        let real = dir.join("real.json");
        let link = dir.join(".claude.json");
        let _ = std::fs::remove_file(&link);
        std::fs::write(
            &real,
            r#"{"projects": {"/a": {}}, "mcpServers": {"x": {}}, "oauthAccount": {"emailAddress": "a@x.com"}}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_identity(&link, &account_with_email("b@x.com")).unwrap();

        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&real).unwrap()).unwrap();
        assert_eq!(v["oauthAccount"]["emailAddress"], "b@x.com");
        assert!(v["projects"]["/a"].is_object() && v["mcpServers"]["x"].is_object());
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    }

    #[test]
    fn secret_goes_over_stdin_as_hex_while_it_fits() {
        let SecretWrite::Stdin(line) = secret_write_command(SERVICE, r#"{"a":"\"q\""}"#, "me")
        else {
            panic!("a small secret must not go through argv");
        };
        assert_eq!(
            line,
            "add-generic-password -U -s \"Claude Code-credentials\" -a \"me\" \
             -X \"7b2261223a225c22715c22227d\"\n"
        );
    }

    #[test]
    fn oversized_secret_falls_back_to_argv_instead_of_being_truncated() {
        // The real item: ~3.7 KB of JSON, most of it `mcpOAuth`.
        let secret = format!(r#"{{"mcpOAuth":"{}"}}"#, "x".repeat(3700));
        match secret_write_command(SERVICE, &secret, "me") {
            SecretWrite::Argv(args) => {
                let hex = args.last().unwrap();
                assert_eq!(hex.len(), secret.len() * 2);
                assert_eq!(&args[..2], ["add-generic-password", "-U"]);
            }
            SecretWrite::Stdin(line) => panic!("{}-byte line would be truncated", line.len()),
        }
    }

    /// Round-trips through the real keychain under a throwaway service name.
    /// `cargo test -- --ignored keychain_roundtrip`
    #[test]
    #[ignore]
    fn keychain_roundtrip_small_and_oversized() {
        let service = "clyde-test-roundtrip";
        for n in [10, 3700, 6000] {
            let secret = format!(r#"{{"mcpOAuth":"{}","q":"\"é\""}}"#, "x".repeat(n));
            write_secret_to(service, &secret, "clyde-test").unwrap();
            let read = read_secret_from(service);
            let _ = Command::new("security")
                .args(["delete-generic-password", "-s", service, "-a", "clyde-test"])
                .output();
            assert_eq!(read.unwrap().as_deref(), Some(secret.as_str()), "n={n}");
        }
    }

    #[test]
    fn merge_credential_updates_tokens_but_preserves_everything_else() {
        let mut root: Map<String, Value> = serde_json::from_str(
            r#"{
                "claudeAiOauth": {
                    "accessToken": "old-a", "refreshToken": "old-r", "expiresAt": 1,
                    "scopes": ["user:inference"],
                    "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"
                },
                "mcpOAuth": { "keep": "me" }
            }"#,
        )
        .unwrap();
        let cred = Credential {
            access_token: "new-a".into(),
            refresh_token: "new-r".into(),
            expires_at: 99,
            scopes: vec![], // a refresh response that omitted scopes
        };
        merge_credential(&mut root, &cred);

        let o = root.get("claudeAiOauth").unwrap();
        assert_eq!(o.get("accessToken").unwrap(), "new-a");
        assert_eq!(o.get("refreshToken").unwrap(), "new-r");
        assert_eq!(o.get("expiresAt").unwrap(), 99);
        // Empty scopes must not wipe the blob's existing list.
        assert_eq!(o.get("scopes").unwrap(), &json!(["user:inference"]));
        // Plan metadata and sibling top-level keys survive a token rotation.
        assert_eq!(o.get("subscriptionType").unwrap(), "max");
        assert_eq!(root.get("mcpOAuth").unwrap().get("keep").unwrap(), "me");
    }

    #[test]
    fn strips_clyde_proxy_keys_but_keeps_everything_else() {
        let dir = tmp("strip");
        let settings = dir.join("settings.json");
        let backup = dir.join(".clyde-integration-backup.json");
        std::fs::write(&backup, "{}").unwrap();
        std::fs::write(
            &settings,
            r#"{
                "apiKeyHelper": "echo clyde-managed-token",
                "effortLevel": "high",
                "env": {
                    "ANTHROPIC_BASE_URL": "http://127.0.0.1:8787",
                    "CLAUDE_CODE_MAX_OUTPUT_TOKENS": "64000"
                }
            }"#,
        )
        .unwrap();

        assert!(cleanup_legacy_at(&settings, &backup).unwrap());

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(v.get("apiKeyHelper").is_none());
        let env = v.get("env").unwrap().as_object().unwrap();
        assert!(env.get("ANTHROPIC_BASE_URL").is_none());
        assert_eq!(env.get("CLAUDE_CODE_MAX_OUTPUT_TOKENS").unwrap(), "64000");
        assert_eq!(v.get("effortLevel").unwrap(), "high");
        assert!(!backup.exists(), "stale backup should be removed");

        // Idempotent: a now-clean file is left alone.
        assert!(!cleanup_legacy_at(&settings, &backup).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaves_a_users_own_apikeyhelper_and_base_url_untouched() {
        let dir = tmp("preserve");
        let settings = dir.join("settings.json");
        let backup = dir.join("b.json");
        // A genuine user/corporate config must survive — only Clyde's own
        // localhost proxy marker is removed.
        std::fs::write(
            &settings,
            r#"{"apiKeyHelper":"echo my-own-helper","env":{"ANTHROPIC_BASE_URL":"https://corp.proxy.example"}}"#,
        )
        .unwrap();

        assert!(!cleanup_legacy_at(&settings, &backup).unwrap());

        let v: Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v.get("apiKeyHelper").unwrap(), "echo my-own-helper");
        assert_eq!(
            v.get("env").unwrap().get("ANTHROPIC_BASE_URL").unwrap(),
            "https://corp.proxy.example"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
