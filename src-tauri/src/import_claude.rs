//! Import accounts directly from Claude Code's own credential store.
//!
//! Claude Code keeps each account's OAuth blob in the macOS Keychain under
//! service `Claude Code-credentials` (the default config dir) or
//! `Claude Code-credentials-<sha256(absConfigDir)[..8]>` for any dir set via
//! `CLAUDE_CONFIG_DIR`. The secret is JSON:
//!
//! ```json
//! { "claudeAiOauth": { "accessToken", "refreshToken", "expiresAt",
//!                       "scopes", "subscriptionType", "rateLimitTier" } }
//! ```
//!
//! This is the reliable onboarding path — no browser OAuth needed; we reuse the
//! tokens Claude Code already minted.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::{now_ms, Account, Credential};

/// A Claude Code account Clyde found but hasn't imported yet.
#[derive(Serialize, Clone, Debug)]
pub struct Discovered {
    /// Stable identifier we'll use for the account (derived from email/dir).
    pub id: String,
    /// Identifies the source config dir (passed back to `import`).
    pub config_dir: String,
    pub label: String,
    pub email: Option<String>,
    pub subscription_type: Option<String>,
}

fn home() -> Result<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| anyhow!("no home directory"))
}

/// Keychain service name for a config dir. The default dir is unsuffixed; any
/// other dir is suffixed with the first 8 hex chars of sha256(absolute path).
fn service_for(dir: &Path, is_default: bool) -> String {
    if is_default {
        return "Claude Code-credentials".to_string();
    }
    let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("Claude Code-credentials-{hex}")
}

/// Where Claude Code stores `.claude.json` for a config dir: the home root for
/// the default account, or inside the dir for a `CLAUDE_CONFIG_DIR` account.
fn account_json_path(dir: &Path, is_default: bool) -> Result<PathBuf> {
    if is_default {
        Ok(home()?.join(".claude.json"))
    } else {
        Ok(dir.join(".claude.json"))
    }
}

fn read_keychain_secret(service: &str) -> Option<String> {
    let out = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Plan metadata pulled alongside the credential from a Claude Code blob.
#[derive(Default, Clone)]
struct PlanMeta {
    rate_limit_tier: Option<String>,
    subscription: Option<String>,
}

fn parse_credential(secret: &str) -> Result<(Credential, PlanMeta)> {
    let v: Value = serde_json::from_str(secret)?;
    let c = v.get("claudeAiOauth").unwrap_or(&v);

    let access = c
        .get("accessToken")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("no accessToken in keychain secret"))?
        .to_string();
    let refresh = c
        .get("refreshToken")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let expires_at = c
        .get("expiresAt")
        .and_then(|x| x.as_i64())
        .unwrap_or_else(now_ms);
    let scopes = c
        .get("scopes")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let rate_limit_tier = c
        .get("rateLimitTier")
        .and_then(|x| x.as_str())
        .map(String::from);
    let subscription = c
        .get("subscriptionType")
        .and_then(|x| x.as_str())
        .map(String::from);

    Ok((
        Credential {
            access_token: access,
            refresh_token: refresh,
            expires_at,
            scopes,
        },
        PlanMeta {
            rate_limit_tier,
            subscription,
        },
    ))
}

/// Turn a rate-limit tier string like `default_claude_max_20x` into a friendly
/// plan label like `Max 20×`.
pub(crate) fn plan_label(tier: &str) -> String {
    let base = if tier.contains("max") {
        "Max"
    } else if tier.contains("pro") {
        "Pro"
    } else {
        "Claude"
    };
    if tier.contains("20x") {
        format!("{base} 20×")
    } else if tier.contains("5x") {
        format!("{base} 5×")
    } else {
        base.to_string()
    }
}

/// Prefer the tier from `.claude.json`, fall back to the keychain blob's tier.
fn subscription_label(meta_tier: &Option<String>, kc_tier: &Option<String>) -> Option<String> {
    meta_tier.as_deref().or(kc_tier.as_deref()).map(plan_label)
}

/// Capture the raw `oauthAccount` object from a `.claude.json`, so Clyde can
/// restore this account's exact identity when it makes it active.
fn read_oauth_account(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("oauthAccount").filter(|o| o.is_object()).cloned()
}

/// Pull email + subscription from a `.claude.json` if present.
fn read_account_meta(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return (None, None);
    };
    let oa = v.get("oauthAccount");
    let email = oa
        .and_then(|o| o.get("emailAddress"))
        .and_then(|x| x.as_str())
        .map(String::from);
    let tier = oa
        .and_then(|o| o.get("organizationRateLimitTier"))
        .and_then(|x| x.as_str())
        .map(String::from);
    (email, tier)
}

/// Candidate config dirs to probe: the default `~/.claude`, any `~/.claude-*`
/// sibling dirs, and `$CLAUDE_CONFIG_DIR` if set.
fn candidate_dirs() -> Result<Vec<(PathBuf, bool)>> {
    let home = home()?;
    let mut out: Vec<(PathBuf, bool)> = vec![(home.join(".claude"), true)];

    if let Ok(entries) = std::fs::read_dir(&home) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".claude-") && entry.path().is_dir() {
                out.push((entry.path(), false));
            }
        }
    }

    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        let p = PathBuf::from(dir);
        let is_default = p == home.join(".claude");
        if !out.iter().any(|(d, _)| d == &p) {
            out.push((p, is_default));
        }
    }

    Ok(out)
}

/// Stable account id for an email, matching what [`discover`]/[`import_account`]
/// produce — so an account added by browser/token login merges with the same
/// account discovered on disk instead of duplicating it.
pub(crate) fn account_id_for_email(email: &str) -> String {
    format!("claude_{}", email.replace(['@', '.'], "_"))
}

fn id_for(email: &Option<String>, dir: &Path) -> String {
    match email {
        Some(e) => account_id_for_email(e),
        None => format!(
            "claude_{}",
            dir.file_name()
                .map(|n| n.to_string_lossy().replace('.', ""))
                .unwrap_or_else(|| "default".into())
        ),
    }
}

fn label_for(email: &Option<String>, dir: &Path, is_default: bool) -> String {
    if let Some(e) = email {
        return e.clone();
    }
    if is_default {
        return "Claude (default)".to_string();
    }
    dir.file_name()
        .map(|n| {
            n.to_string_lossy()
                .trim_start_matches(".claude-")
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Claude account".to_string())
}

/// Find all importable Claude Code accounts (metadata only — no tokens).
pub fn discover() -> Result<Vec<Discovered>> {
    let mut found = Vec::new();
    for (dir, is_default) in candidate_dirs()? {
        let service = service_for(&dir, is_default);
        let Some(secret) = read_keychain_secret(&service) else {
            continue;
        };
        let kc_tier = parse_credential(&secret)
            .ok()
            .and_then(|(_, m)| m.rate_limit_tier);
        let (email, meta_tier) = account_json_path(&dir, is_default)
            .map(|p| read_account_meta(&p))
            .unwrap_or((None, None));
        found.push(Discovered {
            id: id_for(&email, &dir),
            config_dir: dir.to_string_lossy().to_string(),
            label: label_for(&email, &dir, is_default),
            email,
            subscription_type: subscription_label(&meta_tier, &kc_tier),
        });
    }
    Ok(found)
}

/// Import a specific discovered account (by its `config_dir`) into Clyde.
pub fn import_account(config_dir: &str) -> Result<Account> {
    let home = home()?;
    let dir = PathBuf::from(config_dir);
    let is_default = dir == home.join(".claude");
    let service = service_for(&dir, is_default);

    let secret = read_keychain_secret(&service)
        .ok_or_else(|| anyhow!("no Claude Code credentials found for {config_dir}"))?;
    let (credential, plan) = parse_credential(&secret)?;

    let json_path = account_json_path(&dir, is_default).ok();
    let (email, meta_tier) = json_path
        .as_ref()
        .map(|p| read_account_meta(p))
        .unwrap_or((None, None));
    let oauth_account = json_path.as_ref().and_then(|p| read_oauth_account(p));

    Ok(Account {
        id: id_for(&email, &dir),
        label: label_for(&email, &dir, is_default),
        email,
        subscription_type: subscription_label(&meta_tier, &plan.rate_limit_tier),
        subscription_raw: plan.subscription,
        rate_limit_tier: plan.rate_limit_tier,
        credential,
        oauth_account,
        source_config_dir: Some(config_dir.to_string()),
    })
}

/// Whether `config_dir` is the user's *default* `~/.claude`. Clyde rewrites that
/// dir's keychain item on every switch (it's the shared "active account" slot),
/// so it must never be treated as a stable per-account credential source.
pub fn is_default_config_dir(config_dir: &str) -> bool {
    match home() {
        Ok(h) => h.join(".claude") == Path::new(config_dir),
        Err(_) => false,
    }
}
