//! One-click integration with Claude Code via `~/.claude/settings.json`.
//!
//! Enabling Clyde adds exactly two keys, leaving everything else untouched:
//!
//! - `env.ANTHROPIC_BASE_URL` → the local proxy.
//! - `apiKeyHelper` → a throwaway value whose real purpose is to set the auth
//!   *source* to `apiKeyHelper`, which is what lets Claude Code talk to a
//!   non-Anthropic host (the host guard is bypassed for that source). The proxy
//!   supplies the real auth.
//!
//! The prior values of those two keys are saved to a sidecar backup so we can
//! restore the file exactly on disable.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

const HELPER_MARKER: &str = "echo clyde-managed-token";

fn home() -> Result<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .context("locating home directory")
}

fn claude_dir() -> Result<PathBuf> {
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") {
        return Ok(PathBuf::from(d));
    }
    Ok(home()?.join(".claude"))
}

fn settings_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join("settings.json"))
}

fn backup_path() -> Result<PathBuf> {
    Ok(claude_dir()?.join(".clyde-integration-backup.json"))
}

fn read_json(path: &PathBuf) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let text = std::fs::read_to_string(path).context("reading settings.json")?;
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value = serde_json::from_str(&text).context("parsing settings.json")?;
    Ok(value.as_object().cloned().unwrap_or_default())
}

fn write_json(path: &PathBuf, map: &Map<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let pretty = serde_json::to_string_pretty(&Value::Object(map.clone()))?;
    std::fs::write(path, pretty).context("writing settings.json")?;
    Ok(())
}

/// Is settings.json currently pointing Claude Code at our proxy on `port`?
pub fn is_enabled(port: u16) -> bool {
    let Ok(path) = settings_path() else {
        return false;
    };
    let Ok(map) = read_json(&path) else {
        return false;
    };
    map.get("env")
        .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .map(|url| {
            url.contains(&format!("127.0.0.1:{port}")) || url.contains(&format!("localhost:{port}"))
        })
        .unwrap_or(false)
}

/// Wire Claude Code to route through the proxy. Idempotent.
pub fn enable(port: u16) -> Result<()> {
    let path = settings_path()?;
    let mut map = read_json(&path)?;

    // Save a backup of the two keys we touch (only on the first enable).
    let backup = backup_path()?;
    if !backup.exists() {
        let prior = json!({
            "apiKeyHelper": map.get("apiKeyHelper").cloned(),
            "ANTHROPIC_BASE_URL": map
                .get("env")
                .and_then(|e| e.get("ANTHROPIC_BASE_URL"))
                .cloned(),
        });
        write_json(&backup, prior.as_object().unwrap())?;
    }

    map.insert("apiKeyHelper".to_string(), json!(HELPER_MARKER));

    let env_obj = map.entry("env".to_string()).or_insert_with(|| json!({}));
    if !env_obj.is_object() {
        *env_obj = json!({});
    }
    env_obj.as_object_mut().unwrap().insert(
        "ANTHROPIC_BASE_URL".to_string(),
        json!(format!("http://127.0.0.1:{port}")),
    );

    write_json(&path, &map)
}

/// Restore settings.json to its pre-Clyde state.
pub fn disable() -> Result<()> {
    let path = settings_path()?;
    let mut map = read_json(&path)?;
    let backup = backup_path()?;

    let prior = if backup.exists() {
        read_json(&backup)?
    } else {
        Map::new()
    };

    // Restore apiKeyHelper.
    match prior.get("apiKeyHelper") {
        Some(Value::Null) | None => {
            map.remove("apiKeyHelper");
        }
        Some(v) => {
            map.insert("apiKeyHelper".to_string(), v.clone());
        }
    }

    // Restore env.ANTHROPIC_BASE_URL.
    if let Some(env_obj) = map.get_mut("env").and_then(|e| e.as_object_mut()) {
        match prior.get("ANTHROPIC_BASE_URL") {
            Some(Value::Null) | None => {
                env_obj.remove("ANTHROPIC_BASE_URL");
            }
            Some(v) => {
                env_obj.insert("ANTHROPIC_BASE_URL".to_string(), v.clone());
            }
        }
        if env_obj.is_empty() {
            map.remove("env");
        }
    }

    write_json(&path, &map)?;
    std::fs::remove_file(&backup).ok();
    Ok(())
}
