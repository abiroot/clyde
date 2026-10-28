//! A localhost relay between Claude Code and the model providers (Kimi Code,
//! Refuseless), so Claude Code sessions — including running ones — can use them
//! cleanly and cheaply.
//!
//! What it does to each Messages request, and why:
//! - **Unsupported content.** A session that started on Anthropic carries
//!   content the providers reject: Claude Code records tools loaded
//!   mid-conversation as `tool_addition` blocks, and Kimi answers any request
//!   holding one with a bare `400 Invalid request` (verified 2026-10-28 by
//!   replaying a captured request). Clyde can't edit a running session's
//!   history, so the relay drops content the providers don't support.
//! - **Model names.** Claude Code sends whatever model the session runs:
//!   `claude-opus-5-5` from a session that began on Claude, an alias's target,
//!   or another provider's id after a switch. Anything the provider doesn't
//!   serve becomes its chosen model; the provider's own ids pass through, so
//!   `/model k3` in a session works.
//! - **Effort and thinking.** `/effort` arrives as `output_config.effort` with
//!   Claude's five levels. Each model's own levels come from its model list; the
//!   relay sends the highest one at or below the requested level (Claude Code's
//!   own rule), as `reasoning_effort` for Refuseless, which ignores
//!   `output_config`. Thinking switched off becomes `reasoning_effort: off`
//!   where a model allows it; for thinking-only models (all of Kimi's, which
//!   reject it with a 400) the request is sent without the switch.
//! - **Prompt cache.** Both providers cache prompt prefixes automatically. The
//!   attribution line Claude Code puts at the top of the system prompt carries
//!   per-request fields; any change there costs Refuseless the whole cache, so
//!   it's dropped. Refuseless also folds mid-conversation `system` messages
//!   into its system prompt, which changes the prefix every turn (measured: 0
//!   of ~8K tokens cached with one, 7,936 without); the relay sends those as
//!   `<system-reminder>` text on the user side, where they stay in place.
//!   Every rewrite is a pure function of the request, so a conversation's
//!   prefix stays byte-identical from one turn to the next.
//!
//! The relay also holds the keys: settings files get a placeholder token and the
//! relay adds the real one, so keys never sit in a plaintext file.
//!
//! Paths: `/<provider>/…` uses the provider's chosen model for foreign names;
//! `/<provider>/@<model>/…` uses `<model>` (a pinned session's).
//!
//! Deliberately small: HTTP/1.1, one request per connection, `127.0.0.1` only.

use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::provider::{ModelInfo, Provider};

/// Fixed so settings files can name it; unlikely to collide. `CLYDE_RELAY_PORT`
/// overrides it (for running a second, test copy of Clyde).
pub const DEFAULT_PORT: u16 = 47311;

pub fn port() -> u16 {
    static PORT: OnceLock<u16> = OnceLock::new();
    *PORT.get_or_init(|| {
        std::env::var("CLYDE_RELAY_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(DEFAULT_PORT)
    })
}

/// What Claude Code is pointed at for `provider`; `model` pins the model used
/// for names the provider doesn't serve.
pub fn base_url(provider: Provider, model: Option<&str>) -> String {
    match model {
        Some(m) => format!("http://127.0.0.1:{}/{}/@{m}/", port(), provider.id()),
        None => format!("http://127.0.0.1:{}/{}/", port(), provider.id()),
    }
}

/// The provider a relay URL (any Clyde's, any port) routes to.
pub fn provider_of(url: &str) -> Option<Provider> {
    let rest = url
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| url.strip_prefix("http://localhost:"))?;
    let (_port, path) = rest.split_once('/')?;
    Provider::from_id(path.split('/').next()?)
}

/// The token settings files carry; the relay swaps in the real key.
pub const PLACEHOLDER_TOKEN: &str = "clyde-relay";

/// Content block types the providers' Anthropic-compatible APIs accept.
/// Anything else (`tool_addition` today, whatever Anthropic adds next) is
/// dropped.
const SUPPORTED_BLOCKS: &[&str] = &[
    "text",
    "image",
    "document",
    "tool_use",
    "tool_result",
    "thinking",
    "redacted_thinking",
];

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// What the relay needs to know from Clyde, read fresh for every request.
pub trait Directory: Send + Sync {
    fn key(&self, p: Provider) -> Option<String>;
    /// The provider's chosen model.
    fn selected(&self, p: Provider) -> String;
    fn catalog(&self, p: Provider) -> Vec<ModelInfo>;
}

/// Start listening. Fails if the port is taken (e.g. another Clyde).
pub async fn start(http: reqwest::Client, dir: Arc<dyn Directory>) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port()))
        .await
        .with_context(|| format!("binding the relay on 127.0.0.1:{}", port()))?;
    tracing::info!("relay listening on 127.0.0.1:{}", port());
    tauri::async_runtime::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let http = http.clone();
                    let dir = dir.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = serve(stream, &http, dir.as_ref()).await {
                            tracing::debug!("relay request failed: {e:#}");
                        }
                    });
                }
                Err(e) => tracing::warn!("relay accept failed: {e}"),
            }
        }
    });
    Ok(())
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// `/kimi/@k3/v1/messages?beta=true` → (Kimi, Some("k3"), "/v1/messages?beta=true").
fn split_path(path: &str) -> Option<(Provider, Option<String>, String)> {
    let path = path.strip_prefix('/')?;
    let (first, rest) = path.split_once('/').unwrap_or((path, ""));
    let provider = Provider::from_id(first)?;
    if let Some(after) = rest.strip_prefix('@') {
        let (model, rest) = after.split_once('/').unwrap_or((after, ""));
        return Some((provider, Some(model.to_string()), format!("/{rest}")));
    }
    Some((provider, None, format!("/{rest}")))
}

async fn serve(mut stream: TcpStream, http: &reqwest::Client, dir: &dyn Directory) -> Result<()> {
    let req = match read_request(&mut stream).await {
        Ok(r) => r,
        Err(e) => {
            let _ = respond_error(&mut stream, 400, &format!("{e:#}")).await;
            return Err(e);
        }
    };
    let Some((provider, pinned, rest)) = split_path(&req.path) else {
        return respond_error(&mut stream, 404, "not a relay path").await;
    };
    let Some(key) = dir.key(provider) else {
        let msg = format!("{} isn't connected in Clyde", provider.label());
        return respond_error(&mut stream, 401, &msg).await;
    };

    let mut body = req.body;
    if rest.starts_with("/v1/messages") && !body.is_empty() {
        if let Ok(mut json) = serde_json::from_slice::<Value>(&body) {
            let catalog = dir.catalog(provider);
            let fallback = pinned
                .filter(|m| catalog.iter().any(|c| &c.id == m))
                .unwrap_or_else(|| dir.selected(provider));
            let sent = json.get("model").and_then(Value::as_str).map(String::from);
            if rewrite(provider, &catalog, &fallback, &mut json) {
                body = serde_json::to_vec(&json)?;
            }
            let model = json.get("model").and_then(Value::as_str).unwrap_or("?");
            let effort = json.pointer("/output_config/effort").cloned();
            let reasoning = json.get("reasoning_effort").cloned();
            let n = json
                .get("messages")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            tracing::debug!(
                "relay → {}: model {} → {model}, effort {effort:?}/{reasoning:?}, {n} messages",
                provider.label(),
                sent.as_deref().unwrap_or("?"),
            );
        }
    }

    let method = reqwest::Method::from_bytes(req.method.as_bytes())?;
    let mut up = http.request(method, format!("{}{rest}", provider.upstream()));
    for (k, v) in &req.headers {
        let lk = k.to_ascii_lowercase();
        if matches!(
            lk.as_str(),
            "host"
                | "content-length"
                | "connection"
                | "authorization"
                | "x-api-key"
                | "accept-encoding"
        ) {
            continue;
        }
        up = up.header(k, v);
    }
    let mut resp = up.bearer_auth(&key).body(body).send().await?;

    let status = resp.status();
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    );
    for (k, v) in resp.headers() {
        let lk = k.as_str();
        if matches!(
            lk,
            "content-length" | "transfer-encoding" | "connection" | "content-encoding"
        ) {
            continue;
        }
        if let Ok(v) = v.to_str() {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    head.push_str("transfer-encoding: chunked\r\nconnection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    // Stream through as it arrives: replies are server-sent events.
    while let Some(chunk) = resp.chunk().await? {
        stream
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(&chunk).await?;
        stream.write_all(b"\r\n").await?;
        stream.flush().await?;
    }
    stream.write_all(b"0\r\n\r\n").await?;
    stream.flush().await?;
    Ok(())
}

async fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut buf = Vec::with_capacity(8192);
    let header_end = loop {
        let mut chunk = [0u8; 8192];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            bail!("connection closed before the request headers ended");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEADER_BYTES {
            bail!("request headers too large");
        }
    };
    let head = std::str::from_utf8(&buf[..header_end]).context("request headers aren't UTF-8")?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first
        .next()
        .ok_or_else(|| anyhow!("empty request line"))?
        .to_string();
    let path = first
        .next()
        .ok_or_else(|| anyhow!("no request path"))?
        .to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    if header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        bail!("chunked request bodies aren't supported");
    }
    let len: usize = header("content-length")
        .map(|v| v.parse().context("bad content-length"))
        .transpose()?
        .unwrap_or(0);
    if len > MAX_BODY_BYTES {
        bail!("request body too large");
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let mut chunk = vec![0u8; (len - body.len()).min(1 << 20)];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            bail!("connection closed mid-body");
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

async fn respond_error(stream: &mut TcpStream, code: u16, msg: &str) -> Result<()> {
    let body = serde_json::json!({
        "type": "error",
        "error": {"type": "api_error", "message": format!("Clyde relay: {msg}")}
    })
    .to_string();
    let reason = match code {
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    stream
        .write_all(
            format!(
                "HTTP/1.1 {code} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Every rewrite a Messages request gets on its way to `provider` (see the
/// module docs). `fallback` replaces model names the provider doesn't serve.
/// Returns whether anything changed.
pub fn rewrite(
    provider: Provider,
    catalog: &[ModelInfo],
    fallback: &str,
    body: &mut Value,
) -> bool {
    let mut changed = map_model(catalog, fallback, body);
    changed |= sanitize(body);
    changed |= drop_attribution(body);
    if provider.folds_system_messages() {
        changed |= system_messages_to_user(body);
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(fallback);
    if let Some(info) = catalog.iter().find(|m| m.id == model).cloned() {
        changed |= translate_effort(provider, &info, body);
    }
    changed
}

/// Keep the provider's own model ids (minus Claude Code's `[1m]` tag); send
/// anything else as `fallback`.
fn map_model(catalog: &[ModelInfo], fallback: &str, body: &mut Value) -> bool {
    let Some(obj) = body.as_object_mut() else {
        return false;
    };
    let current = obj.get("model").and_then(Value::as_str).unwrap_or("");
    let bare = strip_1m(current);
    let target = if catalog.iter().any(|m| m.id == bare) {
        bare.to_string()
    } else {
        fallback.to_string()
    };
    if target == current {
        return false;
    }
    obj.insert("model".into(), Value::String(target));
    true
}

fn strip_1m(model: &str) -> &str {
    let lower = model.to_ascii_lowercase();
    match lower.strip_suffix("[1m]") {
        Some(_) => &model[..model.len() - 4],
        None => model,
    }
}

/// Drop content blocks the providers reject from a Messages API body, and any
/// message left empty by that. Returns whether anything changed.
pub fn sanitize(body: &mut Value) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    for m in messages.iter_mut() {
        if let Some(content) = m.get_mut("content").and_then(Value::as_array_mut) {
            let before = content.len();
            content.retain(|b| {
                b.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| SUPPORTED_BLOCKS.contains(&t))
            });
            changed |= content.len() != before;
        }
    }
    let before = messages.len();
    messages.retain(|m| {
        m.get("content")
            .map(|c| !matches!(c, Value::Array(a) if a.is_empty()))
            .unwrap_or(false)
    });
    changed | (messages.len() != before)
}

const ATTRIBUTION: &str = "x-anthropic-billing-header:";

/// Remove Claude Code's attribution line from the top of the system prompt.
fn drop_attribution(body: &mut Value) -> bool {
    match body.get_mut("system") {
        Some(Value::Array(blocks)) => {
            let before = blocks.len();
            blocks.retain(|b| {
                !b.get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.starts_with(ATTRIBUTION))
            });
            blocks.len() != before
        }
        Some(Value::String(text)) if text.starts_with(ATTRIBUTION) => {
            *text = text
                .split_once('\n')
                .map(|(_, r)| r.to_string())
                .unwrap_or_default();
            true
        }
        _ => false,
    }
}

/// Send `system`-role messages as `<system-reminder>` text on the user side,
/// merged into the neighbouring user message so roles still alternate.
fn system_messages_to_user(body: &mut Value) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };
    if !messages.iter().any(|m| role(m) == "system") {
        return false;
    }
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    for mut m in std::mem::take(messages) {
        if role(&m) == "system" {
            let blocks = as_blocks(m.get("content"))
                .into_iter()
                .map(|mut b| {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        if !t.trim_start().starts_with("<system-reminder>") {
                            let wrapped = format!("<system-reminder>\n{t}\n</system-reminder>");
                            b["text"] = Value::String(wrapped);
                        }
                    }
                    b
                })
                .collect();
            m = json!({"role": "user", "content": Value::Array(blocks)});
        }
        match out.last_mut() {
            Some(prev) if role(prev) == "user" && role(&m) == "user" => {
                let mut merged = as_blocks(prev.get("content"));
                merged.extend(as_blocks(m.get("content")));
                prev["content"] = Value::Array(merged);
            }
            _ => out.push(m),
        }
    }
    *messages = out;
    true
}

fn role(m: &Value) -> &str {
    m.get("role").and_then(Value::as_str).unwrap_or("")
}

fn as_blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) => vec![json!({"type": "text", "text": s})],
        _ => Vec::new(),
    }
}

/// Claude Code's effort levels, lowest first, with the providers' `off`.
const LEVELS: &[&str] = &["off", "low", "medium", "high", "xhigh", "max"];

/// The highest level `model` offers at or below `wanted` — Claude Code's own
/// rule for models without a level — or its lowest when even that is higher.
pub fn effort_for(model: &ModelInfo, wanted: &str) -> Option<String> {
    let rank = |e: &str| LEVELS.iter().position(|l| *l == e);
    let want = rank(wanted)?;
    let offered: Vec<(usize, &String)> = model
        .efforts
        .iter()
        .filter(|e| e.as_str() != "off")
        .filter_map(|e| Some((rank(e)?, e)))
        .collect();
    offered
        .iter()
        .filter(|(r, _)| *r <= want)
        .max_by_key(|(r, _)| *r)
        .or_else(|| offered.iter().min_by_key(|(r, _)| *r))
        .map(|(_, e)| (*e).clone())
}

fn translate_effort(provider: Provider, model: &ModelInfo, body: &mut Value) -> bool {
    let Some(obj) = body.as_object_mut() else {
        return false;
    };
    let wanted = obj
        .get("output_config")
        .and_then(|o| o.get("effort"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let thinking_off = obj
        .get("thinking")
        .and_then(|t| t.get("type"))
        .and_then(Value::as_str)
        == Some("disabled");
    let level = wanted.as_deref().and_then(|w| effort_for(model, w));
    let mut changed = false;

    if provider.uses_reasoning_effort() {
        // Refuseless reads `reasoning_effort` and ignores `output_config`.
        if wanted.is_some() {
            changed |= remove_effort(obj);
        }
        let re = if thinking_off && model.efforts.iter().any(|e| e == "off") {
            Some("off".to_string())
        } else {
            level
        };
        if let Some(re) = re {
            obj.insert("reasoning_effort".into(), Value::String(re));
            changed = true;
        }
    } else {
        match (wanted.as_deref(), level) {
            (Some(w), Some(l)) if w != l => {
                obj["output_config"]["effort"] = Value::String(l);
                changed = true;
            }
            (Some(_), None) => changed |= remove_effort(obj),
            _ => {}
        }
    }
    // A thinking-only model rejects the switch (Kimi: 400 "does not support
    // disabled thinking mode"); without it the model thinks as it must.
    if thinking_off && model.thinking_only && !provider.uses_reasoning_effort() {
        obj.remove("thinking");
        changed = true;
    }
    changed
}

fn remove_effort(obj: &mut Map<String, Value>) -> bool {
    let Some(Value::Object(oc)) = obj.get_mut("output_config") else {
        return false;
    };
    let removed = oc.remove("effort").is_some();
    if oc.is_empty() {
        obj.remove("output_config");
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, efforts: &[&str]) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            name: id.into(),
            context: 262_144,
            efforts: efforts.iter().map(|e| e.to_string()).collect(),
            thinking_only: !efforts.contains(&"off"),
        }
    }

    #[test]
    fn paths_name_the_provider_and_an_optional_model() {
        assert_eq!(
            split_path("/kimi/v1/messages?beta=true"),
            Some((Provider::Kimi, None, "/v1/messages?beta=true".into()))
        );
        assert_eq!(
            split_path("/refuseless/@glm-5.3-abliterated/v1/messages"),
            Some((
                Provider::Refuseless,
                Some("glm-5.3-abliterated".into()),
                "/v1/messages".into()
            ))
        );
        assert_eq!(split_path("/nope/v1/messages"), None);
        assert_eq!(
            provider_of("http://127.0.0.1:47311/refuseless/@x/"),
            Some(Provider::Refuseless)
        );
        assert_eq!(
            provider_of("http://127.0.0.1:9999/kimi/"),
            Some(Provider::Kimi)
        );
        assert_eq!(provider_of("https://api.kimi.ai/coding/"), None);
        assert_eq!(provider_of("http://127.0.0.1:8080/"), None);
    }

    #[test]
    fn maps_foreign_models_and_keeps_the_providers_own() {
        let catalog = vec![
            info("k3.1-pro", &["low", "high", "max"]),
            info("k3", &["low", "high", "max"]),
        ];
        let mut b = json!({"model": "claude-opus-5-5"});
        assert!(map_model(&catalog, "k3.1-pro", &mut b));
        assert_eq!(b["model"], "k3.1-pro");
        let mut b = json!({"model": "k3[1m]"});
        assert!(map_model(&catalog, "k3.1-pro", &mut b));
        assert_eq!(b["model"], "k3");
        let mut b = json!({"model": "k3"});
        assert!(!map_model(&catalog, "k3.1-pro", &mut b));
    }

    #[test]
    fn efforts_round_down_to_what_the_model_offers() {
        let m = info("glm", &["low", "high", "max"]);
        assert_eq!(effort_for(&m, "medium").as_deref(), Some("low"));
        assert_eq!(effort_for(&m, "high").as_deref(), Some("high"));
        assert_eq!(effort_for(&m, "xhigh").as_deref(), Some("high"));
        assert_eq!(effort_for(&m, "max").as_deref(), Some("max"));
        let only_high = info("x", &["high", "max"]);
        assert_eq!(effort_for(&only_high, "low").as_deref(), Some("high"));
        assert_eq!(effort_for(&info("none", &[]), "high"), None);
        assert_eq!(effort_for(&m, "bogus"), None);
    }

    #[test]
    fn refuseless_gets_reasoning_effort_and_can_turn_thinking_off() {
        let ds = info("deepseek", &["off", "low", "high", "max"]);
        let mut b = json!({"model": "deepseek", "output_config": {"effort": "xhigh"}, "thinking": {"type": "adaptive"}});
        assert!(translate_effort(Provider::Refuseless, &ds, &mut b));
        assert_eq!(b["reasoning_effort"], "high");
        assert!(b.get("output_config").is_none());
        let mut b = json!({"model": "deepseek", "output_config": {"effort": "high", "format": {}}, "thinking": {"type": "disabled"}});
        translate_effort(Provider::Refuseless, &ds, &mut b);
        assert_eq!(b["reasoning_effort"], "off");
        assert_eq!(
            b["output_config"],
            json!({"format": {}}),
            "other output settings stay"
        );
    }

    #[test]
    fn kimi_keeps_output_config_and_never_gets_thinking_disabled() {
        let k = info("k3.1-pro", &["low", "high", "max"]);
        let mut b = json!({"model": "k3.1-pro", "output_config": {"effort": "medium"}, "thinking": {"type": "disabled"}});
        assert!(translate_effort(Provider::Kimi, &k, &mut b));
        assert_eq!(b["output_config"]["effort"], "low");
        assert!(b.get("thinking").is_none());
        assert!(b.get("reasoning_effort").is_none());
        let mut b = json!({"model": "k3.1-pro", "output_config": {"effort": "high"}, "thinking": {"type": "adaptive"}});
        assert!(
            !translate_effort(Provider::Kimi, &k, &mut b),
            "already valid: untouched"
        );
    }

    #[test]
    fn refuseless_system_messages_become_reminders_in_place() {
        let mut b = json!({"messages": [
            {"role": "user", "content": "hi"},
            {"role": "system", "content": "# Environment"},
            {"role": "assistant", "content": [{"type": "text", "text": "ok"}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": "x"}]},
            {"role": "system", "content": [{"type": "text", "text": "<total_tokens>9</total_tokens>", "cache_control": {"type": "ephemeral"}}]}
        ]});
        assert!(system_messages_to_user(&mut b));
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 3);
        assert!(m.iter().all(|m| m["role"] != "system"));
        assert_eq!(
            m[0]["content"][1]["text"],
            "<system-reminder>\n# Environment\n</system-reminder>"
        );
        assert_eq!(
            m[2]["content"][0]["type"], "tool_result",
            "tool results stay first"
        );
        assert_eq!(m[2]["content"][1]["cache_control"]["type"], "ephemeral");
        // Same history, one turn later: the earlier part renders identically.
        let mut longer = json!({"messages": [
            {"role": "user", "content": "hi"},
            {"role": "system", "content": "# Environment"},
            {"role": "assistant", "content": [{"type": "text", "text": "ok"}]},
            {"role": "user", "content": "next"}
        ]});
        system_messages_to_user(&mut longer);
        assert_eq!(longer["messages"][0], m[0]);
    }

    #[test]
    fn drops_the_attribution_line_only() {
        let mut b = json!({"system": [
            {"type": "text", "text": "x-anthropic-billing-header: cc_version=2.1.312; cc_prompt_id=abc;"},
            {"type": "text", "text": "You are Claude Code", "cache_control": {"type": "ephemeral"}}
        ]});
        assert!(drop_attribution(&mut b));
        assert_eq!(b["system"].as_array().unwrap().len(), 1);
        assert!(!drop_attribution(&mut b));
    }

    #[test]
    fn rewrite_leaves_a_clean_kimi_request_alone() {
        let catalog = vec![info("k3.1-pro", &["low", "high", "max"])];
        let mut b = json!({"model": "k3.1-pro", "system": [{"type": "text", "text": "s"}],
            "messages": [{"role": "user", "content": "hi"}, {"role": "system", "content": "env"}],
            "output_config": {"effort": "high"}});
        assert!(
            !rewrite(Provider::Kimi, &catalog, "k3.1-pro", &mut b),
            "Kimi keeps system messages"
        );
    }

    #[test]
    fn drops_tool_additions_and_emptied_messages() {
        let mut body = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
            {"role": "system", "content": [{"type": "text", "text": "note"}, {"type": "tool_addition", "tools": []}]},
            {"role": "system", "content": [{"type": "tool_addition", "tools": []}]},
            {"role": "assistant", "content": [{"type": "thinking", "thinking": "x", "signature": "s"}, {"type": "text", "text": "ok"}]},
            {"role": "user", "content": "plain string stays"}
        ]});
        assert!(sanitize(&mut body));
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "the all-tool_addition message is gone");
        assert_eq!(msgs[1]["content"].as_array().unwrap().len(), 1);
        assert_eq!(
            msgs[2]["content"].as_array().unwrap().len(),
            2,
            "thinking + text kept"
        );
        assert_eq!(msgs[3]["content"], json!("plain string stays"));
        let mut clean = json!({"messages": [{"role": "user", "content": "hi"}]});
        assert!(!sanitize(&mut clean));
    }

    #[tokio::test]
    async fn parses_a_request_and_its_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut s = TcpStream::connect(addr).await.unwrap();
            let body = r#"{"messages":[]}"#;
            let req = format!(
                "POST /kimi/v1/messages?beta=true HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nX-Api-Key: k\r\n\r\n{body}",
                body.len()
            );
            s.write_all(req.as_bytes()).await.unwrap();
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        let req = read_request(&mut stream).await.unwrap();
        client.await.unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/kimi/v1/messages?beta=true");
        assert_eq!(req.body, br#"{"messages":[]}"#);
        assert!(req.headers.iter().any(|(k, _)| k == "X-Api-Key"));
    }
}
