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
        .unwrap_or_else(|_| "https://platform.claude.com/v1/oauth/token".to_string())
}

fn authorize_url_base() -> String {
    // Claude Code's subscription (claude.ai) login authorizes here. Anthropic
    // moved this from `https://claude.ai/oauth/authorize` to the path below; the
    // old host now rejects the request with "Invalid request format".
    std::env::var("CLYDE_OAUTH_AUTHORIZE_URL")
        .unwrap_or_else(|_| "https://claude.com/cai/oauth/authorize".to_string())
}

fn redirect_uri() -> String {
    std::env::var("CLYDE_OAUTH_REDIRECT_URI")
        .unwrap_or_else(|_| "https://platform.claude.com/oauth/code/callback".to_string())
}

fn api_base() -> String {
    std::env::var("CLYDE_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

// Exact scope set Claude Code requests for its subscription login (`U68` in the
// binary, verified live). The order and full set matter: requesting a subset
// against `claude.com/cai` makes the grant fail with "Invalid request format".
const SCOPES: &str =
    "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

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

    // 32 bytes, matching Claude Code. claude.ai's authorize grant rejects a
    // shorter state (Clyde used to send 16 bytes) with "Invalid request format".
    let state = {
        let mut s = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut s);
        b64url(&s)
    };

    // Build with proper form-encoding (spaces → `+`, colons → `%3A`), exactly
    // like Claude Code's `URLSearchParams`. The `claude.com/cai` authorize
    // endpoint requires this — a hand-rolled query with raw colons / `%20`
    // separators is what made the grant fail with "Invalid request format".
    let cid = client_id();
    let redirect = redirect_uri();
    let authorize_url = reqwest::Url::parse_with_params(
        &authorize_url_base(),
        &[
            ("code", "true"),
            ("client_id", cid.as_str()),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("scope", SCOPES),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
        ],
    )
    .context("building authorize URL")?
    .to_string();

    Ok(PkceChallenge {
        verifier,
        authorize_url,
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
    /// `fallback_scopes` covers responses that omit the `scope` field — storing
    /// an empty scope list (and later writing it into Claude Code's keychain)
    /// would make Claude Code see a scopeless session.
    fn into_credential(self, fallback_scopes: &[String]) -> Credential {
        let expires_at = now_ms() + self.expires_in.unwrap_or(3600) * 1000;
        let scopes = match self.scope.as_deref() {
            Some(s) if !s.trim().is_empty() => s.split_whitespace().map(String::from).collect(),
            _ => fallback_scopes.to_vec(),
        };
        Credential {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at,
            scopes,
        }
    }
}

/// The scope set Clyde requests, as a list — the fallback when a token response
/// doesn't echo the granted scopes back.
fn default_scopes() -> Vec<String> {
    SCOPES.split_whitespace().map(String::from).collect()
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
    Ok(token.into_credential(&default_scopes()))
}

/// Use a refresh token to obtain a fresh access token. `fallback_scopes` is the
/// credential's current scope list, preserved when the response omits `scope`.
pub async fn refresh(
    http: &reqwest::Client,
    refresh_token: &str,
    fallback_scopes: &[String],
) -> Result<Credential> {
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
    let fallback = if fallback_scopes.is_empty() {
        default_scopes()
    } else {
        fallback_scopes.to_vec()
    };
    Ok(token.into_credential(&fallback))
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_response_without_scope_keeps_the_old_scopes() {
        let resp = TokenResponse {
            access_token: "new-access".into(),
            refresh_token: "new-refresh".into(),
            expires_in: Some(3600),
            scope: None,
        };
        let old = vec!["user:inference".to_string(), "user:profile".to_string()];
        let cred = resp.into_credential(&old);
        assert_eq!(
            cred.scopes, old,
            "omitted scope must not wipe the stored scopes"
        );

        let resp = TokenResponse {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_in: None,
            scope: Some("user:inference".into()),
        };
        let cred = resp.into_credential(&old);
        assert_eq!(cred.scopes, vec!["user:inference".to_string()]);
    }

    #[test]
    fn authorize_url_matches_claude_code_shape() {
        let c = begin_login().unwrap();
        let u = &c.authorize_url;
        // Right endpoint (claude.com/cai, not the rejecting claude.ai host).
        assert!(
            u.starts_with("https://claude.com/cai/oauth/authorize?"),
            "{u}"
        );
        // Form-encoded scope: `+` separators and `%3A` colons (not raw `%20`/`:`).
        assert!(
            u.contains("scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload"),
            "scope wrongly encoded: {u}"
        );
        // Redirect URI fully percent-encoded.
        assert!(
            u.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"),
            "{u}"
        );
        assert!(
            u.contains("code=true") && u.contains("code_challenge_method=S256"),
            "{u}"
        );
        // 32-byte state (43 base64url chars). A shorter state makes claude.ai's
        // grant fail with "Invalid request format".
        let state = u.rsplit("state=").next().unwrap();
        assert_eq!(
            state.len(),
            43,
            "state must be 32 bytes (43 chars): {state:?}"
        );
    }
}

/// Identity for an account, read from `GET /api/oauth/profile`. Claude's access
/// tokens are opaque (not JWTs), so this endpoint — the same one Claude Code
/// uses — is the only way to learn an account's email and plan from a token.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub email: Option<String>,
    pub full_name: Option<String>,
    /// Raw `subscriptionType` Claude Code stores: `"max"` or `"pro"`.
    pub subscription_raw: Option<String>,
    /// e.g. `"default_claude_max_20x"`.
    pub rate_limit_tier: Option<String>,
}

/// Look up an account's identity from its access token. Best-effort: any network
/// or shape error surfaces as `Err`, and callers fall back to what they have.
pub async fn fetch_profile(http: &reqwest::Client, access_token: &str) -> Result<Profile> {
    let url = format!("{}/api/oauth/profile", api_base().trim_end_matches('/'));
    let resp = http
        .get(url)
        .header("authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .context("requesting /api/oauth/profile")?;
    if !resp.status().is_success() {
        return Err(anyhow!("profile endpoint returned {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.context("parsing profile response")?;
    let account = v.get("account");
    let org = v.get("organization");

    let str_at = |obj: Option<&serde_json::Value>, key: &str| {
        obj.and_then(|o| o.get(key))
            .and_then(|x| x.as_str())
            .map(String::from)
    };
    let bool_at = |obj: Option<&serde_json::Value>, key: &str| {
        obj.and_then(|o| o.get(key)).and_then(|x| x.as_bool())
    };

    let subscription_raw = if bool_at(account, "has_claude_max") == Some(true) {
        Some("max".to_string())
    } else if bool_at(account, "has_claude_pro") == Some(true) {
        Some("pro".to_string())
    } else {
        None
    };

    Ok(Profile {
        email: str_at(account, "email"),
        full_name: str_at(account, "full_name").or_else(|| str_at(account, "display_name")),
        subscription_raw,
        rate_limit_tier: str_at(org, "rate_limit_tier"),
    })
}
