//! Kimi Code (Moonshot) as a provider Claude Code can be switched to.
//!
//! Kimi serves an Anthropic-compatible endpoint for its Kimi Code membership,
//! billed against the plan's quota (a 5-hour window and a monthly cap), not per
//! token. Claude Code reaches it through the `env` block of
//! `~/.claude/settings.json`: base URL, the Kimi Code key as a bearer token, and
//! every model alias pointed at Kimi's model.
//!
//! That block is also how a switch reaches sessions that are already running:
//! Claude Code watches `settings.json`, re-applies its `env` into the running
//! process when it changes, and builds a fresh API client for every request
//! (verified against Claude Code 2.1.312 with live sessions). Two consequences
//! shape [`env_on`] / [`env_off`]:
//! - Deleting a key doesn't unset it in a running session (the env is merged in,
//!   never cleared), so switching back writes **empty** values, which Claude
//!   Code treats as unset. [`tidy`] removes them once no session is running.
//! - The model follows only through aliases (`opus`, `sonnet`, …); a session
//!   launched with a full model id keeps sending it, which Kimi also accepts.
//!
//! The key lives in the Keychain (service [`KEY_SERVICE`]). Settings.json holds
//! it in plaintext only while Kimi is active, as Kimi's own setup does.

use anyhow::{anyhow, bail, Context, Result};
use keyring::Entry;
use serde_json::Value;

use crate::model::{now_ms, UsageLimit, UsageSnapshot};

/// The pseudo account id Kimi appears under in the account list.
pub const ID: &str = "kimi";

/// Keychain item holding the Kimi Code key.
pub const KEY_SERVICE: &str = "Clyde Kimi";
const KEY_ACCOUNT: &str = "kimi-code";

/// Anthropic-compatible endpoint for Kimi Code memberships.
pub const BASE_URL: &str = "https://api.kimi.ai/coding/";

/// Plan usage (5-hour window, monthly caps). Undocumented but used by Kimi's
/// own tools; only answers on the `.com` host.
const USAGE_URL: &str = "https://api.kimi.com/coding/v1/usages";

/// Kimi's coding model.
pub const MODEL: &str = "k3.1-pro";

/// Context limit every Kimi Code plan allows (Pro and above allow 1M).
const CONTEXT_TOKENS: &str = "262144";

/// Kimi's API rejects some generic clients; identify as Clyde.
const USER_AGENT: &str = concat!("Clyde/", env!("CARGO_PKG_VERSION"));

/// Every `env` key Clyde writes to route Claude Code to Kimi. Switching back
/// blanks exactly these.
pub const ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_EFFORT_LEVEL",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
];

// ---- key storage ---------------------------------------------------------

fn entry() -> Result<Entry> {
    Entry::new(KEY_SERVICE, KEY_ACCOUNT).context("opening the Kimi keychain entry")
}

/// The stored Kimi Code key, if Kimi is connected.
pub fn key() -> Option<String> {
    entry()
        .ok()?
        .get_password()
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

pub fn save_key(key: &str) -> Result<()> {
    entry()?
        .set_password(key)
        .context("saving the Kimi key to the Keychain")
}

pub fn delete_key() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("removing the Kimi key from the Keychain"),
    }
}

/// Accept a pasted key with or without an `ANTHROPIC_*=` prefix or quotes.
pub fn normalize_key(raw: &str) -> Result<String> {
    let k = raw.trim();
    let k = k.rsplit_once('=').map(|(_, v)| v).unwrap_or(k);
    let k = k.trim().trim_matches(['"', '\'']).trim();
    if !k.starts_with("sk-") || k.len() < 20 || k.contains(char::is_whitespace) {
        bail!("that doesn't look like a Kimi Code key (it starts with sk-kimi-)");
    }
    Ok(k.to_string())
}

// ---- settings.json env ---------------------------------------------------

/// The `env` entries that make Claude Code use Kimi.
/// The `env` entries that make Claude Code use Kimi through Clyde's relay
/// ([`crate::kimi_relay`]): running sessions can move over mid-conversation,
/// and the key stays out of settings.json.
pub fn env_on_relay() -> Vec<(&'static str, String)> {
    env_with(
        &crate::kimi_relay::base_url(),
        crate::kimi_relay::PLACEHOLDER_TOKEN,
    )
}

/// The same, straight to Kimi with the real key — for when Clyde isn't there
/// to relay (it's quitting). New sessions work; running ones that began on
/// Anthropic may hit Kimi's 400 until restarted.
pub fn env_on_direct(key: &str) -> Vec<(&'static str, String)> {
    env_with(BASE_URL, key)
}

fn env_with(base_url: &str, token: &str) -> Vec<(&'static str, String)> {
    ENV_KEYS
        .iter()
        .map(|&k| {
            let v = match k {
                "ANTHROPIC_BASE_URL" => base_url,
                "ANTHROPIC_AUTH_TOKEN" => token,
                "CLAUDE_CODE_EFFORT_LEVEL" => "high",
                "CLAUDE_CODE_AUTO_COMPACT_WINDOW" | "CLAUDE_CODE_MAX_CONTEXT_TOKENS" => {
                    CONTEXT_TOKENS
                }
                _ => MODEL,
            };
            (k, v.to_string())
        })
        .collect()
}

/// The same keys, blanked: running sessions read "" as unset.
pub fn env_off() -> Vec<(&'static str, String)> {
    ENV_KEYS.iter().map(|&k| (k, String::new())).collect()
}

/// Whether a settings `env` object currently routes Claude Code to Kimi.
pub fn routes_to_kimi(env: &serde_json::Map<String, Value>) -> bool {
    let s = |k: &str| env.get(k).and_then(Value::as_str).unwrap_or("");
    let base = s("ANTHROPIC_BASE_URL");
    (base.contains("api.kimi.ai") || base.contains("api.kimi.com") || via_relay(env))
        && !s("ANTHROPIC_AUTH_TOKEN").is_empty()
}

/// Whether the env points Claude Code at Clyde's own Kimi relay.
pub fn via_relay(env: &serde_json::Map<String, Value>) -> bool {
    env.get("ANTHROPIC_BASE_URL")
        .and_then(Value::as_str)
        .is_some_and(|b| {
            b.trim_end_matches('/') == crate::kimi_relay::base_url().trim_end_matches('/')
        })
}

/// Keys [`env_off`] blanked that can now go, because no session needs to see
/// them cleared any more.
pub fn tidy(env: &mut serde_json::Map<String, Value>) -> bool {
    let before = env.len();
    env.retain(|k, v| !(ENV_KEYS.contains(&k.as_str()) && v.as_str() == Some("")));
    env.len() != before
}

// ---- usage -----------------------------------------------------------------

/// Read the plan's usage. An auth failure means the key was revoked.
pub async fn fetch_usage(http: &reqwest::Client, key: &str) -> Result<UsageSnapshot> {
    let resp = http
        .get(USAGE_URL)
        .bearer_auth(key)
        .header("user-agent", USER_AGENT)
        .send()
        .await?;
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        bail!("usage endpoint returned {status}");
    }
    if !resp.status().is_success() {
        bail!("usage endpoint returned {status}");
    }
    let body: Value = resp.json().await?;
    parse_usage(&body).ok_or_else(|| anyhow!("unexpected Kimi usage response"))
}

/// Check a key before saving it: Kimi must accept it.
pub async fn verify_key(http: &reqwest::Client, key: &str) -> Result<UsageSnapshot> {
    fetch_usage(http, key).await.map_err(|e| {
        if format!("{e}").contains("401") || format!("{e}").contains("403") {
            anyhow!("Kimi didn't accept that key — copy it again from the Kimi Code console")
        } else {
            e.context("couldn't reach Kimi to check the key")
        }
    })
}

/// `{"usages": {"limit_5h": {"used_ratio": 0.016, "reset_time": "…"},
///   "limit_month_total": {…}, "limit_month_code": {…}}, "limits": [...]}`
/// → the 5-hour window as the session limit and the monthly cap as a scoped
/// limit. Ratios are 0..=1.
pub fn parse_usage(body: &Value) -> Option<UsageSnapshot> {
    let usages = body.get("usages")?;
    let ratio = |k: &str| {
        usages
            .pointer(&format!("/{k}/used_ratio"))
            .and_then(Value::as_f64)
            .map(|r| (r * 100.0).clamp(0.0, 100.0))
    };
    let reset = |k: &str| {
        usages
            .pointer(&format!("/{k}/reset_time"))
            .and_then(Value::as_str)
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp())
    };
    let five = ratio("limit_5h");
    // The coding cap and the overall cap both bind; show whichever is tighter.
    let month = [
        ("limit_month_code", ratio("limit_month_code")),
        ("limit_month_total", ratio("limit_month_total")),
    ]
    .into_iter()
    .filter_map(|(k, p)| p.map(|p| (k, p)))
    .max_by(|a, b| a.1.total_cmp(&b.1));
    if five.is_none() && month.is_none() {
        return None;
    }
    let scoped_limits = month
        .map(|(k, p)| {
            vec![UsageLimit {
                label: "Monthly · Kimi".into(),
                percent: p,
                severity: None,
                resets_at: reset(k),
            }]
        })
        .unwrap_or_default();
    let five_hour_resets_at = reset("limit_5h");
    let resets_at = [
        five_hour_resets_at,
        scoped_limits.first().and_then(|l| l.resets_at),
    ]
    .into_iter()
    .flatten()
    .min();
    let full = five.is_some_and(|p| p >= 100.0) || scoped_limits.iter().any(|l| l.percent >= 100.0);
    Some(UsageSnapshot {
        five_hour_utilization: five,
        seven_day_utilization: None,
        status: full.then(|| "rejected".to_string()),
        resets_at,
        five_hour_resets_at,
        seven_day_resets_at: None,
        updated_at: now_ms(),
        scoped_limits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_the_live_usage_shape() {
        let body = json!({
            "limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                        "detail": {"limit": "100", "used": "2", "remaining": "98", "resetTime": "2026-10-28T04:26:19.166120Z"}}],
            "usages": {
                "limit_5h": {"used_ratio": 0.0164, "reset_time": "2026-10-28T04:26:18Z"},
                "limit_month_total": {"used_ratio": 0.003, "reset_time": "2026-11-28T00:00:00Z"},
                "limit_month_code": {"used_ratio": 0.002, "reset_time": "2026-11-28T00:00:00Z"}
            }
        });
        let u = parse_usage(&body).unwrap();
        assert!((u.five_hour_utilization.unwrap() - 1.64).abs() < 1e-9);
        assert_eq!(u.scoped_limits.len(), 1);
        assert_eq!(u.scoped_limits[0].label, "Monthly · Kimi");
        assert!(
            (u.scoped_limits[0].percent - 0.3).abs() < 1e-9,
            "the tighter monthly cap"
        );
        assert_eq!(u.five_hour_resets_at, Some(1793161578));
        assert!(u.status.is_none());
        assert!(parse_usage(&json!({"error": "x"})).is_none());
    }

    #[test]
    fn a_full_window_reads_as_rejected() {
        let body = json!({"usages": {"limit_5h": {"used_ratio": 1.0}}});
        assert_eq!(
            parse_usage(&body).unwrap().status.as_deref(),
            Some("rejected")
        );
    }

    #[test]
    fn env_on_routes_and_env_off_blanks_the_same_keys() {
        let on: serde_json::Map<String, Value> = env_on_direct("sk-kimi-abc")
            .into_iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        assert!(routes_to_kimi(&on));
        assert!(!via_relay(&on));
        assert_eq!(on["ANTHROPIC_DEFAULT_OPUS_MODEL"], json!(MODEL));
        assert_eq!(on["ANTHROPIC_AUTH_TOKEN"], json!("sk-kimi-abc"));

        let relay: serde_json::Map<String, Value> = env_on_relay()
            .into_iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        assert!(routes_to_kimi(&relay) && via_relay(&relay));
        assert!(
            !relay["ANTHROPIC_AUTH_TOKEN"]
                .as_str()
                .unwrap()
                .starts_with("sk-"),
            "no key in the file"
        );

        let mut off: serde_json::Map<String, Value> = env_off()
            .into_iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        assert!(!routes_to_kimi(&off));
        assert_eq!(off.len(), ENV_KEYS.len());
        off.insert("CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(), json!("64000"));
        assert!(tidy(&mut off));
        assert_eq!(off.len(), 1, "tidy keeps the user's own keys");
    }

    #[test]
    fn normalizes_pasted_keys() {
        assert_eq!(
            normalize_key("  sk-kimi-0123456789abcdef  ").unwrap(),
            "sk-kimi-0123456789abcdef"
        );
        assert_eq!(
            normalize_key("ANTHROPIC_API_KEY=\"sk-kimi-0123456789abcdef\"").unwrap(),
            "sk-kimi-0123456789abcdef"
        );
        assert!(normalize_key("hello").is_err());
    }
}
