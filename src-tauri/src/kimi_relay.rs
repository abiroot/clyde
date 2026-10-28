//! A localhost relay between Claude Code and Kimi, so running sessions can
//! move to Kimi mid-conversation.
//!
//! A session that started on Anthropic carries content Kimi rejects: Claude
//! Code records tools loaded mid-conversation (MCP servers, tool search) as
//! `tool_addition` blocks in the history, and Kimi answers any request holding
//! one with a bare `400 Invalid request` (verified 2026-10-28 by replaying a
//! captured request: dropping only those blocks makes it pass). A session that
//! starts on Kimi never produces them. Clyde can't edit a running session's
//! history, so while Kimi is active Claude Code talks to this relay instead,
//! which removes content Kimi doesn't support and forwards the rest.
//!
//! The relay also holds the key: settings.json gets a placeholder token and the
//! relay adds the real one, so the key never sits in a plaintext file.
//!
//! Deliberately small: HTTP/1.1, one request per connection, `127.0.0.1` only.

use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Fixed so settings.json can name it; unlikely to collide.
pub const PORT: u16 = 47311;

/// Path prefix the relay answers under (also how Clyde recognises its own URL).
pub const PREFIX: &str = "/kimi";

/// What Claude Code is pointed at while Kimi is active.
pub fn base_url() -> String {
    format!("http://127.0.0.1:{PORT}{PREFIX}/")
}

/// The token settings.json carries; the relay swaps in the real key.
pub const PLACEHOLDER_TOKEN: &str = "clyde-kimi-relay";

/// Where requests go.
const UPSTREAM: &str = "https://api.kimi.ai/coding";

/// Content block types Kimi's Anthropic-compatible API accepts. Anything else
/// (`tool_addition` today, whatever Anthropic adds next) is dropped.
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

pub type KeySource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Start listening. Fails if the port is taken (e.g. another Clyde).
pub async fn start(http: reqwest::Client, key: KeySource) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", PORT))
        .await
        .with_context(|| format!("binding the Kimi relay on 127.0.0.1:{PORT}"))?;
    tracing::info!("Kimi relay listening on {}", base_url());
    tauri::async_runtime::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let http = http.clone();
                    let key = key.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = serve(stream, &http, &key).await {
                            tracing::debug!("Kimi relay request failed: {e:#}");
                        }
                    });
                }
                Err(e) => tracing::warn!("Kimi relay accept failed: {e}"),
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

async fn serve(mut stream: TcpStream, http: &reqwest::Client, key: &KeySource) -> Result<()> {
    let req = match read_request(&mut stream).await {
        Ok(r) => r,
        Err(e) => {
            let _ = respond_error(&mut stream, 400, &format!("{e:#}")).await;
            return Err(e);
        }
    };
    let Some(rest) = req.path.strip_prefix(PREFIX) else {
        return respond_error(&mut stream, 404, "not a Kimi relay path").await;
    };
    let Some(key) = key() else {
        return respond_error(&mut stream, 401, "Kimi isn't connected in Clyde").await;
    };

    let mut body = req.body;
    if rest.starts_with("/v1/messages") && !body.is_empty() {
        if let Ok(mut json) = serde_json::from_slice::<Value>(&body) {
            if sanitize(&mut json) {
                body = serde_json::to_vec(&json)?;
            }
        }
    }

    let method = reqwest::Method::from_bytes(req.method.as_bytes())?;
    let mut up = http.request(method, format!("{UPSTREAM}{rest}"));
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
        "error": {"type": "api_error", "message": format!("Clyde Kimi relay: {msg}")}
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

/// Drop content blocks Kimi rejects from a Messages API body, and any message
/// left empty by that. Returns whether anything changed.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
