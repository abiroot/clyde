//! The transparent auth proxy.
//!
//! Claude Code points `ANTHROPIC_BASE_URL` at this server (on localhost). For
//! every request we:
//!   1. pick the active account,
//!   2. mint a fresh `Authorization: Bearer` + `anthropic-beta: oauth-…`,
//!   3. forward to `api.anthropic.com`, streaming the response straight back,
//!   4. read the `anthropic-ratelimit-unified-*` headers to update usage,
//!   5. on a 429 / hard limit, transparently retry on the next account.
//!
//! Because the proxy supplies the real OAuth headers itself, the credential
//! Claude Code sends is irrelevant — which is what lets a single config dir
//! (one settings.json) drive any number of accounts.

use std::collections::HashSet;
use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use axum::routing::any;
use axum::Router;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};

use crate::engine::SharedCore;
use crate::ratelimit;

const OAUTH_BETA: &str = "oauth-2025-04-20";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Hop-by-hop / auth headers we never forward upstream verbatim.
const STRIP: &[&str] = &[
    "host",
    "authorization",
    "x-api-key",
    "content-length",
    "connection",
    "proxy-connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "accept-encoding",
];

fn upstream_base() -> String {
    std::env::var("CLYDE_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

/// Bind the proxy and serve forever. Flips the engine's `proxy_running` flag.
pub async fn run(core: SharedCore, port: u16) -> anyhow::Result<()> {
    let app = Router::new().fallback(any(handle)).with_state(core.clone());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("clyde proxy listening on http://{addr}");
    core.set_proxy_running(true);

    let result = axum::serve(listener, app).await;
    core.set_proxy_running(false);
    result.map_err(Into::into)
}

async fn handle(State(core): State<SharedCore>, req: Request) -> Response {
    match proxy_request(core, req).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("proxy error: {e:#}");
            error_response(StatusCode::BAD_GATEWAY, &format!("Clyde proxy error: {e}"))
        }
    }
}

async fn proxy_request(core: SharedCore, req: Request) -> anyhow::Result<Response> {
    let (parts, body) = req.into_parts();
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_BYTES).await?;

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let url = format!(
        "{}{}",
        upstream_base().trim_end_matches('/'),
        path_and_query
    );

    let Some(first) = core.choose_account() else {
        return Ok(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "No Claude account configured in Clyde. Open Clyde and add an account.",
        ));
    };

    let mut tried: HashSet<String> = HashSet::new();
    let mut account_id = first;

    loop {
        let bearer = match core.valid_bearer(&account_id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("token refresh failed for {account_id}: {e:#}");
                tried.insert(account_id.clone());
                match core.choose_failover_excluding(&collect(&tried)) {
                    Some(next) => {
                        account_id = next;
                        continue;
                    }
                    None => {
                        return Ok(error_response(
                            StatusCode::BAD_GATEWAY,
                            &format!("Could not obtain a valid token: {e}"),
                        ))
                    }
                }
            }
        };

        let upstream = build_upstream(
            &core,
            &parts.method,
            &url,
            &parts.headers,
            &body_bytes,
            &bearer,
        )?;
        let resp = core.http.execute(upstream).await?;

        let status = resp.status();
        let headers = resp.headers().clone();

        if let Some(snap) = ratelimit::parse(&headers) {
            core.record_usage(&account_id, snap);
        }

        if ratelimit::is_rate_limited(status, &headers) {
            tracing::info!("account {account_id} rate-limited; attempting failover");
            core.mark_limited(&account_id);
            tried.insert(account_id.clone());
            if let Some(next) = core.choose_failover_excluding(&collect(&tried)) {
                account_id = next;
                continue;
            }
            // No account left — return the upstream limit response as-is.
        }

        return stream_back(status, headers, resp);
    }
}

fn build_upstream(
    core: &SharedCore,
    method: &http::Method,
    url: &str,
    incoming: &HeaderMap,
    body: &[u8],
    bearer: &str,
) -> anyhow::Result<reqwest::Request> {
    let mut builder = core.http.request(method.clone(), url).body(body.to_vec());

    // Forward client headers except hop-by-hop / auth ones.
    let mut out = HeaderMap::new();
    for (name, value) in incoming.iter() {
        if STRIP.contains(&name.as_str()) {
            continue;
        }
        out.insert(name.clone(), value.clone());
    }

    // Inject the real subscription auth.
    out.insert(
        http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {bearer}"))?,
    );

    // Ensure the oauth beta flag is present (merge with any client-sent betas).
    let beta = match incoming.get("anthropic-beta").and_then(|v| v.to_str().ok()) {
        Some(existing) if !existing.contains(OAUTH_BETA) => format!("{existing},{OAUTH_BETA}"),
        Some(existing) => existing.to_string(),
        None => OAUTH_BETA.to_string(),
    };
    out.insert("anthropic-beta", HeaderValue::from_str(&beta)?);

    if !out.contains_key("anthropic-version") {
        out.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
    }

    builder = builder.headers(out);
    Ok(builder.build()?)
}

/// Stream the upstream response body straight back to Claude Code without
/// buffering — essential for SSE token streaming to stay low-latency.
fn stream_back(
    status: StatusCode,
    headers: HeaderMap,
    resp: reqwest::Response,
) -> anyhow::Result<Response> {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers.iter() {
        // Let the runtime recompute framing headers.
        if matches!(
            name.as_str(),
            "transfer-encoding" | "content-length" | "connection"
        ) {
            continue;
        }
        builder = builder.header(name, value);
    }
    let stream = resp.bytes_stream();
    Ok(builder.body(Body::from_stream(stream))?)
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let body = serde_json::json!({
        "type": "error",
        "error": { "type": "clyde_proxy_error", "message": message }
    });
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn collect(set: &HashSet<String>) -> Vec<String> {
    set.iter().cloned().collect()
}
