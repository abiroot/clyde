//! Claude OAuth: refreshing access tokens and driving the PKCE login flow.
//!
//! Clyde manages each account's tokens itself so it can mint a fresh
//! `Authorization: Bearer` for every proxied request. The client id and
//! endpoints below mirror Claude Code's own public OAuth client; override them
//! with the `CLYDE_OAUTH_*` environment variables if Anthropic changes them.
//!
//! NOTE: these values are reverse-engineered from Claude Code's public OAuth
//! flow and may change without notice — they are not secrets, but they are not
//! a stable API either. See ROADMAP.md.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::model::{now_ms, Credential};

fn client_id() -> String {
    std::env::var("CLYDE_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string())
}

fn token_url() -> String {
    std::env::var("CLYDE_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://console.anthropic.com/v1/oauth/token".to_string())
}

fn authorize_url_base() -> String {
    std::env::var("CLYDE_OAUTH_AUTHORIZE_URL")
        .unwrap_or_else(|_| "https://claude.ai/oauth/authorize".to_string())
}

fn redirect_uri() -> String {
    std::env::var("CLYDE_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| "https://console.anthropic.com/oauth/code/callback".to_string())
}

const SCOPES: &str = "org:create_api_key user:profile user:inference";

/// One in-flight PKCE login attempt. Hold onto `verifier` until the user pastes
/// back the authorization code, then call [`exchange_code`].
pub struct PkceChallenge {
    pub verifier: String,
    pub authorize_url: String,
}

/// Begin a login: build the authorize URL the user should open in a browser.
pub fn begin_login() -> Result<PkceChallenge> {
    let mut verifier_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let verifier = b64url(&verifier_bytes);

    let challenge = {
        let digest = Sha256::digest(verifier.as_bytes());
        b64url(&digest)
    };

    let state = {
        let mut s = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut s);
        b64url(&s)
    };

    let mut url = url::Url::parse(&authorize_url_base()).context("parsing authorize url")?;
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", &client_id())
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri())
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    Ok(PkceChallenge {
        verifier,
        authorize_url: url.to_string(),
    })
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

impl TokenResponse {
    fn into_credential(self) -> Credential {
        let expires_at = now_ms() + self.expires_in.unwrap_or(3600) * 1000;
        Credential {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at,
            scopes: self
                .scope
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
        }
    }
}

/// Exchange the authorization code (the user pastes `code#state`) for tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    code_and_state: &str,
    verifier: &str,
) -> Result<Credential> {
    let (code, state) = match code_and_state.split_once('#') {
        Some((c, s)) => (c.trim(), Some(s.trim())),
        None => (code_and_state.trim(), None),
    };

    let mut body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "client_id": client_id(),
        "redirect_uri": redirect_uri(),
        "code_verifier": verifier,
    });
    if let Some(state) = state {
        body["state"] = serde_json::Value::String(state.to_string());
    }

    let resp = http
        .post(token_url())
        .json(&body)
        .send()
        .await
        .context("sending token exchange request")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("token exchange failed ({status}): {text}"));
    }

    let token: TokenResponse = resp.json().await.context("parsing token response")?;
    Ok(token.into_credential())
}

/// Use a refresh token to obtain a fresh access token.
pub async fn refresh(http: &reqwest::Client, refresh_token: &str) -> Result<Credential> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": client_id(),
    });

    let resp = http
        .post(token_url())
        .json(&body)
        .send()
        .await
        .context("sending refresh request")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("token refresh failed ({status}): {text}"));
    }

    let mut token: TokenResponse = resp.json().await.context("parsing refresh response")?;
    // Some refresh responses omit a new refresh token; keep the old one.
    if token.refresh_token.is_empty() {
        token.refresh_token = refresh_token.to_string();
    }
    Ok(token.into_credential())
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
