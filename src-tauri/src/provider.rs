//! Model providers Claude Code can be switched to besides Claude accounts:
//! Kimi Code (Moonshot) and Refuseless.
//!
//! Both serve an Anthropic-compatible Messages endpoint. Kimi Code is billed
//! against a membership's quota (a 5-hour window and a monthly cap); Refuseless
//! charges a prepaid credit balance per token. Claude Code reaches either
//! through the `env` block of a settings file: base URL, a token, and every
//! model alias pointed at the provider's chosen model.
//!
//! That block is also how a switch reaches sessions that are already running:
//! Claude Code watches its settings files, re-applies their `env` into the
//! running process when it changes, and builds a fresh API client for every
//! request (verified against Claude Code 2.1.312 with live sessions). Two
//! consequences shape [`env_on`] / [`env_off`]:
//! - Deleting a key doesn't unset it in a running session (the env is merged in,
//!   never cleared), so switching back writes **empty** values, which Claude
//!   Code treats as unset. [`tidy`] removes them once no session is running.
//! - The model follows only through aliases (`opus`, `sonnet`, …). A session
//!   that picked a model by id keeps sending it, so the relay maps any model the
//!   provider doesn't serve to its chosen one ([`crate::relay`]).
//!
//! Keys live in the Keychain. With Clyde's relay running, settings files only
//! ever hold a placeholder token.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::model::{now_ms, UsageLimit, UsageSnapshot};

/// Kimi's and Refuseless's APIs reject some generic clients; identify as Clyde.
const USER_AGENT: &str = concat!("Clyde/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Kimi,
    Refuseless,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Kimi, Provider::Refuseless];

    /// The pseudo account id the provider appears under in the account list
    /// (and its relay path prefix, without the slash).
    pub fn id(self) -> &'static str {
        match self {
            Provider::Kimi => "kimi",
            Provider::Refuseless => "refuseless",
        }
    }

    pub fn from_id(id: &str) -> Option<Provider> {
        Provider::ALL
            .into_iter()
            .find(|p| p.id().eq_ignore_ascii_case(id.trim()))
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Kimi => "Kimi",
            Provider::Refuseless => "Refuseless",
        }
    }

    /// Anthropic-compatible API root (Claude Code appends `/v1/messages`).
    pub fn upstream(self) -> String {
        #[cfg(debug_assertions)]
        if let Some(u) = test_hook(&format!("CLYDE_TEST_UPSTREAM_{}", self.id().to_uppercase())) {
            return u;
        }
        match self {
            Provider::Kimi => "https://api.kimi.ai/coding",
            Provider::Refuseless => "https://api.refuseless.com",
        }
        .to_string()
    }

    fn key_entry(self) -> Result<Entry> {
        let (service, account) = match self {
            Provider::Kimi => ("Clyde Kimi", "kimi-code"),
            Provider::Refuseless => ("Clyde Refuseless", "refuseless"),
        };
        Entry::new(service, account).context("opening the provider's keychain entry")
    }

    /// The model used until the user picks one.
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Kimi => "k3.1-pro",
            Provider::Refuseless => "deepseek-v4.1-flash-abliterated",
        }
    }

    /// Whether the provider charges per token (rather than against a plan).
    /// Auto-switch leaves these out until the user opts in.
    pub fn pay_per_token(self) -> bool {
        matches!(self, Provider::Refuseless)
    }

    /// Mid-conversation `system` messages cost the whole prompt cache: the
    /// provider folds them into the system prompt, so the prefix changes on
    /// every turn (measured 2026-10-28: 7,936 of 7,984 tokens cached without
    /// one, 0 with one). The relay sends them as user-side reminders instead.
    pub fn folds_system_messages(self) -> bool {
        matches!(self, Provider::Refuseless)
    }

    /// Effort goes in Refuseless's own `reasoning_effort` field; Kimi reads
    /// Anthropic's `output_config.effort`.
    pub fn uses_reasoning_effort(self) -> bool {
        matches!(self, Provider::Refuseless)
    }

    /// Accept a pasted key with or without an `ANTHROPIC_*=` prefix or quotes.
    pub fn normalize_key(self, raw: &str) -> Result<String> {
        let k = raw.trim();
        let k = k.rsplit_once('=').map(|(_, v)| v).unwrap_or(k);
        let k = k.trim().trim_matches(['"', '\'']).trim();
        let (prefix, hint) = match self {
            Provider::Kimi => ("sk-", "a Kimi Code key (it starts with sk-kimi-)"),
            Provider::Refuseless => ("abl_", "a Refuseless key (it starts with abl_)"),
        };
        if !k.starts_with(prefix) || k.len() < 20 || k.contains(char::is_whitespace) {
            bail!("that doesn't look like {hint}");
        }
        Ok(k.to_string())
    }

    // ---- key storage ------------------------------------------------------

    /// The stored key, if the provider is connected.
    pub fn key(self) -> Option<String> {
        #[cfg(debug_assertions)]
        if let Some(keys) = test_keys() {
            return keys
                .get(self.id())
                .and_then(Value::as_str)
                .map(String::from);
        }
        self.key_entry()
            .ok()?
            .get_password()
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
    }

    pub fn save_key(self, key: &str) -> Result<()> {
        #[cfg(debug_assertions)]
        if test_keys().is_some() {
            return Ok(());
        }
        self.key_entry()?
            .set_password(key)
            .context("saving the key to the Keychain")
    }

    /// Delete and re-save the key, so the Keychain item is created by Clyde
    /// itself (and its access rules follow Clyde, like the account list's).
    /// Best-effort.
    pub fn adopt_key(self, key: &str) {
        if self.delete_key().and_then(|_| self.save_key(key)).is_err() {
            // If the re-save failed after the delete, try once more so the key
            // isn't lost.
            if let Err(e) = self.save_key(key) {
                tracing::warn!("couldn't re-save the {} key: {e:#}", self.label());
            }
        }
    }

    pub fn delete_key(self) -> Result<()> {
        #[cfg(debug_assertions)]
        if test_keys().is_some() {
            return Ok(());
        }
        match self.key_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("removing the key from the Keychain"),
        }
    }

    // ---- models -----------------------------------------------------------

    /// What to offer before the live list has ever been read.
    pub fn fallback_models(self) -> Vec<ModelInfo> {
        let m = |id: &str, name: &str, context: u64, efforts: &[&str]| ModelInfo {
            id: id.into(),
            name: name.into(),
            context,
            efforts: efforts.iter().map(|e| e.to_string()).collect(),
            thinking_only: !efforts.contains(&"off"),
        };
        match self {
            Provider::Kimi => vec![m("k3.1-pro", "K3.1 Pro", 262_144, &["low", "high", "max"])],
            Provider::Refuseless => vec![m(
                "deepseek-v4.1-flash-abliterated",
                "DeepSeek V4.1 Flash Abliterated",
                1_048_576,
                &["off", "low", "high", "max"],
            )],
        }
    }

    /// The provider's live model list (`GET /v1/models`).
    pub async fn fetch_models(self, http: &reqwest::Client, key: &str) -> Result<Vec<ModelInfo>> {
        let resp = http
            .get(format!("{}/v1/models", self.upstream()))
            .bearer_auth(key)
            .header("user-agent", USER_AGENT)
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("models endpoint returned {}", resp.status().as_u16());
        }
        let body: Value = resp.json().await?;
        let models = parse_models(&body);
        if models.is_empty() {
            bail!("the provider listed no models");
        }
        Ok(models)
    }

    // ---- usage ------------------------------------------------------------

    /// Read the plan's usage (Kimi) or the credit balance (Refuseless). An auth
    /// failure means the key was revoked.
    pub async fn fetch_usage(self, http: &reqwest::Client, key: &str) -> Result<UsageSnapshot> {
        let url = match self {
            // Undocumented but used by Kimi's own tools; only answers on `.com`.
            Provider::Kimi => "https://api.kimi.com/coding/v1/usages",
            // Free: no credits are charged and no hold is placed.
            Provider::Refuseless => "https://api.refuseless.com/v1/credits",
        };
        let resp = http
            .get(url)
            .bearer_auth(key)
            .header("user-agent", USER_AGENT)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            bail!("usage endpoint returned {status}");
        }
        let body: Value = resp.json().await?;
        let parsed = match self {
            Provider::Kimi => parse_kimi_usage(&body),
            Provider::Refuseless => parse_credits(&body),
        };
        parsed.ok_or_else(|| anyhow!("unexpected {} usage response", self.label()))
    }

    /// Check a key before saving it: the provider must accept it.
    pub async fn verify_key(self, http: &reqwest::Client, key: &str) -> Result<UsageSnapshot> {
        self.fetch_usage(http, key).await.map_err(|e| {
            let msg = format!("{e}");
            if msg.contains("401") || msg.contains("403") {
                anyhow!(
                    "{} didn't accept that key — copy it again from its console",
                    self.label()
                )
            } else {
                e.context(format!("couldn't reach {} to check the key", self.label()))
            }
        })
    }
}

/// Debug builds only: an env var for running a test copy of Clyde next to the
/// real one, without touching the real one's Keychain items.
#[cfg(debug_assertions)]
fn test_hook(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// Debug builds only: `CLYDE_TEST_KEYS={"kimi": "…"}` stands in for the Keychain.
#[cfg(debug_assertions)]
fn test_keys() -> Option<Map<String, Value>> {
    serde_json::from_str(&test_hook("CLYDE_TEST_KEYS")?).ok()
}

/// One model a provider serves, as Clyde shows and routes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    /// Context window in tokens.
    pub context: u64,
    /// Reasoning levels the model accepts, in the provider's words
    /// (`off`, `low`, `high`, `max`). Empty: no effort control.
    #[serde(default)]
    pub efforts: Vec<String>,
    /// Thinking can't be turned off.
    #[serde(default)]
    pub thinking_only: bool,
}

/// Parse `GET /v1/models` from either provider. Kimi names its models and
/// nests reasoning details; Refuseless gives ids and a flat effort list.
pub fn parse_models(body: &Value) -> Vec<ModelInfo> {
    let Some(data) = body.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let strings = |ptr: &str| -> Option<Vec<String>> {
                m.pointer(ptr)?.as_array().map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
            };
            let efforts = strings("/reasoning_efforts")
                .or_else(|| strings("/reasoning/effort/valid"))
                .or_else(|| strings("/think_efforts/valid_efforts"))
                .unwrap_or_default();
            let thinking_only = m.get("supports_thinking_type").and_then(Value::as_str)
                == Some("only")
                || m.pointer("/reasoning/type").and_then(Value::as_str) == Some("only")
                || (!efforts.is_empty() && !efforts.iter().any(|e| e == "off"));
            let name = m
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(String::from)
                .unwrap_or_else(|| pretty_model_name(&id));
            let context = m
                .get("context_length")
                .or_else(|| m.pointer("/limit/context"))
                .and_then(Value::as_u64)
                .unwrap_or(200_000);
            Some(ModelInfo {
                id,
                name,
                context,
                efforts,
                thinking_only,
            })
        })
        .collect()
}

/// `glm-5.3-flash-abliterated` → `GLM 5.3 Flash Abliterated`.
pub fn pretty_model_name(id: &str) -> String {
    id.split('-')
        .map(|w| match w {
            "glm" => "GLM".to_string(),
            "deepseek" => "DeepSeek".to_string(),
            "minimax" => "MiniMax".to_string(),
            "qwen" => "Qwen".to_string(),
            w if w.starts_with('v') && w[1..].starts_with(|c: char| c.is_ascii_digit()) => {
                format!("V{}", &w[1..])
            }
            w if w.starts_with(|c: char| c.is_ascii_digit()) => w.to_string(),
            w => {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Model lists, kept between launches so the picker isn't empty offline.
pub fn load_catalogs(data_dir: &Path) -> std::collections::HashMap<Provider, Vec<ModelInfo>> {
    std::fs::read_to_string(data_dir.join("models.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_catalogs(
    data_dir: &Path,
    catalogs: &std::collections::HashMap<Provider, Vec<ModelInfo>>,
) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("models.json");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(catalogs)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

// ---- settings env -----------------------------------------------------------

/// The family aliases Claude Code resolves through `ANTHROPIC_DEFAULT_*_MODEL`.
const FAMILIES: [&str; 4] = ["FABLE", "OPUS", "SONNET", "HAIKU"];

/// Every `env` key Clyde writes to route Claude Code to a provider. Switching
/// back blanks exactly these. Includes keys Clyde 1.2 wrote and no longer sets,
/// so they're cleared too.
pub const ENV_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
    "ANTHROPIC_DEFAULT_FABLE_MODEL_DESCRIPTION",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
    "ANTHROPIC_DEFAULT_OPUS_MODEL_DESCRIPTION",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
    "ANTHROPIC_DEFAULT_SONNET_MODEL_DESCRIPTION",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL_DESCRIPTION",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_EFFORT_LEVEL",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_MODEL_CAPABILITIES",
    "CLAUDE_CODE_ATTRIBUTION_HEADER",
    "CLAUDE_CODE_DISABLE_1M_CONTEXT",
];

/// Compact long conversations at this size at most, even on 1M-token models:
/// every request resends the whole conversation, and Refuseless bills it.
const COMPACT_CAP: u64 = 262_144;

/// Where and how Claude Code reaches a provider.
pub struct Route<'a> {
    pub provider: Provider,
    /// The relay URL (`…/kimi/`) or the provider's own API.
    pub base_url: String,
    /// The relay's placeholder, or the real key when going direct.
    pub token: String,
    /// The model the aliases point at.
    pub model: &'a ModelInfo,
    /// Everything the provider serves (for capabilities and context).
    pub catalog: &'a [ModelInfo],
}

/// The `env` entries that make Claude Code use a provider.
pub fn env_on(r: &Route) -> Vec<(&'static str, String)> {
    let label = r.provider.label();
    let smallest = r
        .catalog
        .iter()
        .map(|m| m.context)
        .chain([r.model.context])
        .min()
        .unwrap_or(200_000);
    ENV_KEYS
        .iter()
        .map(|&k| {
            let v = match k {
                "ANTHROPIC_BASE_URL" => r.base_url.clone(),
                "ANTHROPIC_AUTH_TOKEN" => r.token.clone(),
                "ANTHROPIC_SMALL_FAST_MODEL" => r.model.id.clone(),
                // Unknown model ids get Claude Code's 200K guess, or 1M when it
                // assumes the default model is a 1M one; give it the real window.
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS" => smallest.to_string(),
                "CLAUDE_CODE_AUTO_COMPACT_WINDOW" => smallest.min(COMPACT_CAP).to_string(),
                "CLAUDE_CODE_DISABLE_1M_CONTEXT" => "1".into(),
                "CLAUDE_CODE_MODEL_CAPABILITIES" => capabilities(r.catalog),
                // Its per-request fields sit at the very top of the system
                // prompt, which costs prefix-cached providers the whole cache.
                "CLAUDE_CODE_ATTRIBUTION_HEADER" => "0".into(),
                k if k.ends_with("_MODEL_NAME") => r.model.name.clone(),
                k if k.ends_with("_MODEL_DESCRIPTION") => {
                    format!("{label} · {}", context_label(r.model.context))
                }
                k if FAMILIES
                    .iter()
                    .any(|f| k == format!("ANTHROPIC_DEFAULT_{f}_MODEL")) =>
                {
                    r.model.id.clone()
                }
                // ANTHROPIC_MODEL would pin sessions launched from now on, the
                // subagent and effort overrides would block `/model`, `/effort`
                // and per-agent models: leave them to Claude Code.
                _ => String::new(),
            };
            (k, v)
        })
        .collect()
}

/// What Claude Code should assume each model supports, so `/effort` and the
/// thinking toggle work on ids it doesn't know. `mid_conv_tool_change` is off:
/// tools loaded mid-conversation then go in the request's tool list, which
/// both providers understand, instead of `tool_addition` blocks they reject.
pub fn capabilities(catalog: &[ModelInfo]) -> String {
    catalog
        .iter()
        .map(|m| {
            let mut caps = vec!["thinking", "adaptive_thinking", "interleaved_thinking"];
            if !m.efforts.is_empty() {
                caps.push("effort");
            }
            if m.efforts.iter().any(|e| e == "max") {
                caps.push("max_effort");
            }
            caps.push("-mid_conv_tool_change");
            format!("{}={}", m.id, caps.join(","))
        })
        .collect::<Vec<_>>()
        .join(";")
}

pub fn context_label(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{}M context", (tokens as f64 / 1_048_576.0).round() as u64)
    } else {
        format!("{}K context", tokens / 1024)
    }
}

/// The same keys, blanked: running sessions read "" as unset.
pub fn env_off() -> Vec<(&'static str, String)> {
    ENV_KEYS.iter().map(|&k| (k, String::new())).collect()
}

/// The `/model` picker lineup while a provider is active: its models under
/// their own names, instead of Claude's.
pub fn model_picker(provider: Provider, catalog: &[ModelInfo]) -> Value {
    let options: Vec<Value> = catalog
        .iter()
        .map(|m| {
            json!({
                "model": m.id,
                "label": m.name,
                "description": format!("{} · {}", provider.label(), context_label(m.context)),
            })
        })
        .collect();
    json!({ "options": options, "replaceBuiltInOptions": true })
}

/// Which provider a settings `env` object routes Claude Code to, if any.
pub fn routed_provider(env: &Map<String, Value>) -> Option<Provider> {
    let s = |k: &str| env.get(k).and_then(Value::as_str).unwrap_or("");
    if s("ANTHROPIC_AUTH_TOKEN").is_empty() {
        return None;
    }
    let base = s("ANTHROPIC_BASE_URL");
    if let Some(p) = crate::relay::provider_of(base) {
        return Some(p);
    }
    let host = base.to_ascii_lowercase();
    if host.contains("api.kimi.ai") || host.contains("api.kimi.com") {
        Some(Provider::Kimi)
    } else if host.contains("api.refuseless.com") {
        Some(Provider::Refuseless)
    } else {
        None
    }
}

/// Keys [`env_off`] blanked that can now go, because no session needs to see
/// them cleared any more.
pub fn tidy(env: &mut Map<String, Value>) -> bool {
    let before = env.len();
    env.retain(|k, v| !(ENV_KEYS.contains(&k.as_str()) && v.as_str() == Some("")));
    env.len() != before
}

// ---- usage parsing -----------------------------------------------------------

/// `{"usages": {"limit_5h": {"used_ratio": 0.016, "reset_time": "…"},
///   "limit_month_total": {…}, "limit_month_code": {…}}, "limits": [...]}`
/// → the 5-hour window as the session limit and the monthly cap as a scoped
/// limit. Ratios are 0..=1.
pub fn parse_kimi_usage(body: &Value) -> Option<UsageSnapshot> {
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
        credit_usd: None,
    })
}

/// `{"object": "credits", "balance": 10.03, "reserved": 0, "available": 10.03,
///   "unit": "usd"}` → no percentages (nothing resets), the spendable balance,
/// and "rejected" once it's gone (requests then fail with 402).
pub fn parse_credits(body: &Value) -> Option<UsageSnapshot> {
    let available = body
        .get("available")
        .or_else(|| body.get("balance"))
        .and_then(Value::as_f64)?;
    Some(UsageSnapshot {
        status: (available <= 0.0).then(|| "rejected".to_string()),
        updated_at: now_ms(),
        credit_usd: Some(available),
        ..UsageSnapshot::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_map(pairs: Vec<(&'static str, String)>) -> Map<String, Value> {
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect()
    }

    #[test]
    fn parses_the_live_kimi_usage_shape() {
        let body = json!({
            "limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                        "detail": {"limit": "100", "used": "2", "remaining": "98", "resetTime": "2026-10-28T04:26:19.166120Z"}}],
            "usages": {
                "limit_5h": {"used_ratio": 0.0164, "reset_time": "2026-10-28T04:26:18Z"},
                "limit_month_total": {"used_ratio": 0.003, "reset_time": "2026-11-28T00:00:00Z"},
                "limit_month_code": {"used_ratio": 0.002, "reset_time": "2026-11-28T00:00:00Z"}
            }
        });
        let u = parse_kimi_usage(&body).unwrap();
        assert!((u.five_hour_utilization.unwrap() - 1.64).abs() < 1e-9);
        assert_eq!(u.scoped_limits.len(), 1);
        assert_eq!(u.scoped_limits[0].label, "Monthly · Kimi");
        assert!(
            (u.scoped_limits[0].percent - 0.3).abs() < 1e-9,
            "the tighter monthly cap"
        );
        assert_eq!(u.five_hour_resets_at, Some(1793161578));
        assert!(u.status.is_none());
        assert!(parse_kimi_usage(&json!({"error": "x"})).is_none());
    }

    #[test]
    fn a_full_window_reads_as_rejected() {
        let body = json!({"usages": {"limit_5h": {"used_ratio": 1.0}}});
        assert_eq!(
            parse_kimi_usage(&body).unwrap().status.as_deref(),
            Some("rejected")
        );
    }

    #[test]
    fn parses_refuseless_credits() {
        let u = parse_credits(
            &json!({"object": "credits", "balance": 10.03, "reserved": 0.5,
            "available": 9.53, "unit": "usd"}),
        )
        .unwrap();
        assert_eq!(u.credit_usd, Some(9.53));
        assert!(u.status.is_none());
        assert!(u.five_hour_utilization.is_none() && u.scoped_limits.is_empty());
        let empty = parse_credits(&json!({"available": 0})).unwrap();
        assert_eq!(empty.status.as_deref(), Some("rejected"));
        assert!(parse_credits(&json!({"error": {}})).is_none());
    }

    #[test]
    fn parses_both_model_lists() {
        let kimi = json!({"data": [
            {"id": "k3.1-pro", "display_name": "K3.1 Pro", "context_length": 262144,
             "supports_thinking_type": "only",
             "reasoning": {"type": "only", "effort": {"valid": ["low", "high", "max"], "default": "high"}}},
            {"id": "kimi-for-coding-highspeed", "display_name": "K2.7 Code Highspeed",
             "context_length": 262144, "reasoning": {"type": "only"}}
        ]});
        let m = parse_models(&kimi);
        assert_eq!(m[0].name, "K3.1 Pro");
        assert_eq!(m[0].efforts, vec!["low", "high", "max"]);
        assert!(m[0].thinking_only);
        assert!(m[1].efforts.is_empty() && m[1].thinking_only);

        let rl = json!({"object": "list", "data": [
            {"id": "glm-5.3-abliterated", "context_length": 1048576,
             "reasoning_efforts": ["low", "high", "max"], "reasoning_default": "low"},
            {"id": "deepseek-v4.1-flash-fast", "context_length": 1048576,
             "reasoning_efforts": ["off", "low", "high", "max"]}
        ]});
        let m = parse_models(&rl);
        assert_eq!(m[0].name, "GLM 5.3 Abliterated");
        assert!(m[0].thinking_only);
        assert_eq!(m[1].name, "DeepSeek V4.1 Flash Fast");
        assert!(!m[1].thinking_only);
        assert_eq!(m[1].context, 1_048_576);
    }

    #[test]
    fn pretty_names() {
        assert_eq!(
            pretty_model_name("minimax-m3.1-flash"),
            "MiniMax M3.1 Flash"
        );
        assert_eq!(
            pretty_model_name("glm-5.3-flash-abliterated"),
            "GLM 5.3 Flash Abliterated"
        );
    }

    #[test]
    fn env_on_routes_and_env_off_blanks_the_same_keys() {
        let catalog = vec![
            ModelInfo {
                id: "k3.1-pro".into(),
                name: "K3.1 Pro".into(),
                context: 262_144,
                efforts: vec!["low".into(), "high".into(), "max".into()],
                thinking_only: true,
            },
            ModelInfo {
                id: "kimi-for-coding".into(),
                name: "K2.8 Preview".into(),
                context: 1_048_576,
                efforts: vec![],
                thinking_only: true,
            },
        ];
        let on = env_map(env_on(&Route {
            provider: Provider::Kimi,
            base_url: crate::relay::base_url(Provider::Kimi, None),
            token: crate::relay::PLACEHOLDER_TOKEN.into(),
            model: &catalog[0],
            catalog: &catalog,
        }));
        assert_eq!(routed_provider(&on), Some(Provider::Kimi));
        assert!(crate::relay::provider_of(on["ANTHROPIC_BASE_URL"].as_str().unwrap()).is_some());
        assert_eq!(on["ANTHROPIC_DEFAULT_OPUS_MODEL"], json!("k3.1-pro"));
        assert_eq!(on["ANTHROPIC_DEFAULT_OPUS_MODEL_NAME"], json!("K3.1 Pro"));
        assert_eq!(on["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], json!("262144"));
        assert_eq!(
            on["CLAUDE_CODE_EFFORT_LEVEL"],
            json!(""),
            "/effort stays usable"
        );
        assert_eq!(on["ANTHROPIC_MODEL"], json!(""));
        assert_eq!(
            on["CLAUDE_CODE_MODEL_CAPABILITIES"],
            json!("k3.1-pro=thinking,adaptive_thinking,interleaved_thinking,effort,max_effort,-mid_conv_tool_change;kimi-for-coding=thinking,adaptive_thinking,interleaved_thinking,-mid_conv_tool_change")
        );
        assert!(!on["ANTHROPIC_AUTH_TOKEN"]
            .as_str()
            .unwrap()
            .starts_with("sk-"));

        let direct = env_map(env_on(&Route {
            provider: Provider::Refuseless,
            base_url: Provider::Refuseless.upstream(),
            token: "abl_x".into(),
            model: &catalog[1],
            catalog: &catalog[1..],
        }));
        assert_eq!(routed_provider(&direct), Some(Provider::Refuseless));
        assert_eq!(direct["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], json!("262144"));
        assert_eq!(direct["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], json!("1048576"));

        let mut off = env_map(env_off());
        assert_eq!(routed_provider(&off), None);
        assert_eq!(off.len(), ENV_KEYS.len());
        off.insert("CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(), json!("64000"));
        assert!(tidy(&mut off));
        assert_eq!(off.len(), 1, "tidy keeps the user's own keys");
    }

    #[test]
    fn recognises_a_clyde_1_2_kimi_route() {
        let mut env = Map::new();
        env.insert(
            "ANTHROPIC_BASE_URL".into(),
            json!("http://127.0.0.1:47311/kimi/"),
        );
        env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!("clyde-kimi-relay"));
        assert_eq!(routed_provider(&env), Some(Provider::Kimi));
        env.insert(
            "ANTHROPIC_BASE_URL".into(),
            json!("https://api.kimi.ai/coding/"),
        );
        assert_eq!(routed_provider(&env), Some(Provider::Kimi));
    }

    #[test]
    fn normalizes_pasted_keys() {
        assert_eq!(
            Provider::Kimi
                .normalize_key("  sk-kimi-0123456789abcdef  ")
                .unwrap(),
            "sk-kimi-0123456789abcdef"
        );
        assert_eq!(
            Provider::Kimi
                .normalize_key("ANTHROPIC_API_KEY=\"sk-kimi-0123456789abcdef\"")
                .unwrap(),
            "sk-kimi-0123456789abcdef"
        );
        assert!(Provider::Kimi.normalize_key("hello").is_err());
        assert!(Provider::Refuseless
            .normalize_key("abl_0123456789abcdef0123")
            .is_ok());
        assert!(Provider::Refuseless
            .normalize_key("sk-kimi-0123456789abcdef")
            .is_err());
    }

    #[test]
    fn context_labels() {
        assert_eq!(context_label(262_144), "256K context");
        assert_eq!(context_label(1_048_576), "1M context");
    }
}
