//! Core domain types shared across the proxy, vault, and Tauri commands.

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
}

/// Live rate-limit picture for an account, parsed from Anthropic's
/// `anthropic-ratelimit-unified-*` response headers.
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

impl UsageSnapshot {
    /// The worst (highest) utilization across tracked windows, used for routing.
    pub fn pressure(&self) -> f64 {
        let a = self.five_hour_utilization.unwrap_or(0.0);
        let b = self.seven_day_utilization.unwrap_or(0.0);
        a.max(b)
    }

    /// True if this account has been rejected (hard limited) right now.
    pub fn is_limited(&self) -> bool {
        matches!(self.status.as_deref(), Some("rejected"))
    }
}

/// How Clyde decides which account a request goes to.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "accountId", rename_all = "snake_case")]
pub enum Mode {
    /// Balance + fail over automatically across all enabled accounts.
    #[default]
    Auto,
    /// Always use this account; never fail over.
    Pinned(String),
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
    pub mode: Mode,
    pub active_id: Option<String>,
    pub proxy_port: u16,
    pub proxy_running: bool,
    /// Whether `~/.claude/settings.json` is currently wired to route through Clyde.
    pub integration_enabled: bool,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
