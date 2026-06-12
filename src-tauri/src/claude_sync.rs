//! Make a Clyde account the one Claude Code uses, by writing its OAuth directly
//! into Claude Code's own credential store — no proxy, no `settings.json` edits.
//!
//! Claude Code (default config dir) reads its subscription token from the macOS
//! Keychain item `Claude Code-credentials`, and shows identity from
//! `~/.claude/.claude.json` → `oauthAccount`. To switch accounts we rewrite both,
//! in place, preserving everything else. Plain `claude` then talks straight to
//! api.anthropic.com as the chosen account — whether or not Clyde is running.
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

fn claude_json_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join(".claude.json"))
}

fn settings_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join("settings.json"))
}

// ---- keychain -------------------------------------------------------------

/// Read the raw `Claude Code-credentials` secret, if present.
fn read_secret() -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", SERVICE, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
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
/// `claude` keeps reading it without a prompt. The command goes through
/// `security -i` (stdin) so the OAuth secret never appears in the process
/// argument list, where any local process could read it via `ps`.
fn write_secret(secret: &str, acct: &str) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    // security(1)'s stdin parser supports double-quoted words with backslash
    // escapes; serde-serialized JSON contains no raw newlines.
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut line = format!("add-generic-password -U -s \"{}\"", esc(SERVICE));
    if !acct.is_empty() {
        line.push_str(&format!(" -a \"{}\"", esc(acct)));
    }
    line.push_str(&format!(" -w \"{}\"\n", esc(secret)));

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
    let out = child
        .wait_with_output()
        .context("waiting for `security -i`")?;
    if !out.status.success() {
        return Err(anyhow!(
            "security add-generic-password failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

// ---- public API -----------------------------------------------------------

/// Make `account` the active Claude Code account: write its OAuth into the
/// keychain and update `.claude.json`'s identity to match.
pub fn activate(account: &Account) -> Result<()> {
    // Preserve any other top-level keys already in the blob (e.g. `mcpOAuth`).
    let mut root: Map<String, Value> = read_secret()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

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

/// The email of the identity Claude Code currently displays, from
/// `~/.claude/.claude.json` → `oauthAccount.emailAddress`. Tokens are opaque,
/// so this is the only offline way to tell *whose* credential the shared
/// keychain slot holds.
pub fn read_active_identity_email() -> Option<String> {
    let path = claude_json_path().ok()?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    v.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(str::to_string)
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
        std::fs::write(path, serde_json::to_string_pretty(&Value::Object(root))?)?;
        let _ = std::fs::remove_file(backup);
    }
    Ok(changed)
}

// ---- .claude.json ---------------------------------------------------------

fn update_claude_json(account: &Account) -> Result<()> {
    let path = claude_json_path()?;
    let mut root: Map<String, Value> = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path).context("reading .claude.json")?)
            .unwrap_or_default()
    } else {
        Map::new()
    };

    let oauth_account = if let Some(meta) = &account.oauth_account {
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

    std::fs::write(&path, serde_json::to_string_pretty(&Value::Object(root))?)
        .context("writing .claude.json")?;
    set_private(&path);
    Ok(())
}

#[cfg(unix)]
fn set_private(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private(_path: &PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("clyde_test_{tag}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
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
