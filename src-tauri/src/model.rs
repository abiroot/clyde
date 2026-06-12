//! Core domain types shared across the engine, vault, and Tauri commands.

use serde::{Deserialize, Serialize};

/// A Claude account Clyde can route through.
///
/// The full struct (including tokens) only ever lives in memory and in the OS
/// keychain. The frontend receives [`AccountView`] instead, which omits secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub email: Option<String>,
    /// "max", "pro", etc. — purely for display.
    #[serde(default)]
    pub subscription_type: Option<String>,
    pub credential: Credential,
    /// Raw `subscriptionType` (e.g. `"max"`, `"pro"`) from Claude Code's
    /// credential blob, captured at import so [`crate::claude_sync::activate`] can
    /// write *this* account's plan back — not the outgoing account's.
    #[serde(default)]
    pub subscription_raw: Option<String>,
    /// Raw `rateLimitTier` (e.g. `"default_claude_max_20x"`) from the credential
    /// blob, written back on activation for the same reason.
    #[serde(default)]
    pub rate_limit_tier: Option<String>,
    /// The `oauthAccount` object Claude Code keeps in `.claude.json`, captured at
    /// import time (when available) so Clyde can restore the right identity when
    /// it makes this account active. Synthesized as `{ "emailAddress": … }` for
    /// accounts added via OAuth login or token paste.
    #[serde(default)]
    pub oauth_account: Option<serde_json::Value>,
    /// The Claude Code config dir this account was imported from, if any. Claude
    /// Code keeps that dir's keychain token fresh, so Clyde re-reads it at switch
    /// time to recover from a stale stored snapshot.
    #[serde(default)]
    pub source_config_dir: Option<String>,
}

/// OAuth credential for a single account.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Credential {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix epoch milliseconds at which `access_token` expires.
    pub expires_at: i64,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl Credential {
    /// True when the access token is expired or within `skew_ms` of expiring.
    pub fn is_stale(&self, now_ms: i64, skew_ms: i64) -> bool {
        self.expires_at - skew_ms <= now_ms
    }

    /// Whether this credential is a *newer generation* than `other`: a genuinely
    /// different token with a later expiry. Anthropic's refresh tokens are
    /// single-use — every refresh rotates the pair and pushes `expires_at`
    /// forward, so the later expiry identifies who refreshed last. Adopting an
    /// older generation means adopting a consumed refresh token, which is a
    /// guaranteed logout the next time it's used.
    pub fn is_newer_than(&self, other: &Credential) -> bool {
        self.expires_at > other.expires_at && self.access_token != other.access_token
    }
}

/// Live usage picture for an account, parsed from Anthropic's
/// `GET /api/oauth/usage` endpoint (the same one Claude Code's status line uses).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    /// 0..=100 utilization of the rolling 5-hour window.
    pub five_hour_utilization: Option<f64>,
    /// 0..=100 utilization of the 7-day window.
    pub seven_day_utilization: Option<f64>,
    /// "allowed" | "rejected" | "queueing" ...
    pub status: Option<String>,
    /// Unix epoch seconds when the most-constrained window resets.
    pub resets_at: Option<i64>,
    /// Unix epoch milliseconds of the last update.
    pub updated_at: i64,
}

/// Secret-free projection of an account for the UI.
#[derive(Clone, Debug, Serialize)]
pub struct AccountView {
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    pub subscription_type: Option<String>,
    pub usage: UsageSnapshot,
    /// Whether this account is the one currently selected to serve traffic.
    pub is_active: bool,
}

/// Snapshot of the whole engine for the UI to render in one shot.
#[derive(Clone, Debug, Serialize)]
pub struct AppSnapshot {
    pub accounts: Vec<AccountView>,
    /// The account Clyde has made active in Claude Code's credential store.
    pub active_id: Option<String>,
    /// Email of the active account, for the title bar.
    pub active_email: Option<String>,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(access: &str, expires_at: i64) -> Credential {
        Credential {
            access_token: access.into(),
            refresh_token: format!("r-{access}"),
            expires_at,
            scopes: vec![],
        }
    }

    #[test]
    fn is_newer_than_requires_a_different_token_with_a_later_expiry() {
        let stored = cred("a1", 1_000);
        assert!(cred("a2", 2_000).is_newer_than(&stored), "later rotation");
        assert!(!cred("a2", 500).is_newer_than(&stored), "older generation");
        assert!(!cred("a1", 2_000).is_newer_than(&stored), "same token");
        assert!(!cred("a2", 1_000).is_newer_than(&stored), "ambiguous tie");
    }
}
