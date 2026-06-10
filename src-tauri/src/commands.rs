//! Tauri commands — the typed bridge the React UI calls into.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use serde::Serialize;
use tauri::State;

use crate::engine::SharedCore;
use crate::model::{now_ms, Account, AppSnapshot, Credential, Mode};
use crate::{claude_config, oauth};

/// In-flight PKCE logins, keyed by an opaque flow id, holding the verifier until
/// the user pastes back their authorization code.
#[derive(Default)]
pub struct PendingLogins(pub Mutex<HashMap<String, String>>);

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
pub fn get_snapshot(core: State<SharedCore>) -> AppSnapshot {
    core.snapshot()
}

#[tauri::command]
pub fn set_mode(core: State<SharedCore>, mode: Mode) -> CmdResult<AppSnapshot> {
    core.set_mode(mode);
    Ok(core.snapshot())
}

#[tauri::command]
pub fn rename_account(
    core: State<SharedCore>,
    id: String,
    label: String,
) -> CmdResult<AppSnapshot> {
    core.rename_account(&id, &label).map_err(err)?;
    Ok(core.snapshot())
}

#[tauri::command]
pub fn remove_account(core: State<SharedCore>, id: String) -> CmdResult<AppSnapshot> {
    core.remove_account(&id).map_err(err)?;
    Ok(core.snapshot())
}

#[derive(Serialize)]
pub struct LoginStart {
    pub flow_id: String,
    pub authorize_url: String,
}

/// Begin a browser OAuth login. The UI opens `authorize_url`, the user signs in,
/// copies the resulting code, and calls `complete_login`.
#[tauri::command]
pub fn begin_login(pending: State<PendingLogins>) -> CmdResult<LoginStart> {
    let challenge = oauth::begin_login().map_err(err)?;
    let flow_id = gen_id("flow");
    pending
        .0
        .lock()
        .unwrap()
        .insert(flow_id.clone(), challenge.verifier);
    Ok(LoginStart {
        flow_id,
        authorize_url: challenge.authorize_url,
    })
}

/// Finish a login: exchange the pasted code for tokens and store the account.
#[tauri::command]
pub async fn complete_login(
    core: State<'_, SharedCore>,
    pending: State<'_, PendingLogins>,
    flow_id: String,
    code: String,
    label: String,
) -> CmdResult<AppSnapshot> {
    let verifier = pending
        .0
        .lock()
        .unwrap()
        .remove(&flow_id)
        .ok_or("login flow expired — start again")?;

    let credential = oauth::exchange_code(&core.http, &code, &verifier)
        .await
        .map_err(err)?;

    let (email, sub) = inspect_token(&credential.access_token);
    let account = Account {
        id: sub.unwrap_or_else(|| gen_id("acc")),
        label: if label.trim().is_empty() {
            email.clone().unwrap_or_else(|| "Account".to_string())
        } else {
            label
        },
        email,
        subscription_type: None,
        credential,
    };

    core.add_account(account).map_err(err)?;
    Ok(core.snapshot())
}

/// Import an already-authenticated session by pasting its token JSON
/// (`{ accessToken, refreshToken, expiresAt, ... }`). A no-OAuth fallback.
#[tauri::command]
pub fn import_token(
    core: State<SharedCore>,
    label: String,
    token_json: String,
) -> CmdResult<AppSnapshot> {
    #[derive(serde::Deserialize)]
    struct Incoming {
        #[serde(alias = "accessToken")]
        access_token: String,
        #[serde(alias = "refreshToken")]
        refresh_token: String,
        #[serde(alias = "expiresAt", default)]
        expires_at: Option<i64>,
        #[serde(default)]
        scopes: Vec<String>,
    }
    let parsed: Incoming = serde_json::from_str(&token_json).map_err(err)?;
    let credential = Credential {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_at: parsed.expires_at.unwrap_or_else(|| now_ms() + 3_600_000),
        scopes: parsed.scopes,
    };
    let (email, sub) = inspect_token(&credential.access_token);
    let account = Account {
        id: sub.unwrap_or_else(|| gen_id("acc")),
        label: if label.trim().is_empty() {
            email
                .clone()
                .unwrap_or_else(|| "Imported account".to_string())
        } else {
            label
        },
        email,
        subscription_type: None,
        credential,
    };
    core.add_account(account).map_err(err)?;
    Ok(core.snapshot())
}

#[tauri::command]
pub fn enable_integration(core: State<SharedCore>) -> CmdResult<AppSnapshot> {
    claude_config::enable(core.proxy_port()).map_err(err)?;
    core.set_integration_enabled(true);
    Ok(core.snapshot())
}

#[tauri::command]
pub fn disable_integration(core: State<SharedCore>) -> CmdResult<AppSnapshot> {
    claude_config::disable().map_err(err)?;
    core.set_integration_enabled(false);
    Ok(core.snapshot())
}

// ---- helpers --------------------------------------------------------------

fn gen_id(prefix: &str) -> String {
    use rand::Rng;
    let n: u64 = rand::thread_rng().gen();
    format!("{prefix}_{:x}{:x}", now_ms(), n)
}

/// Best-effort extraction of email + stable subject from a JWT access token.
/// Returns `(email, subject)` — both optional; never fails.
fn inspect_token(token: &str) -> (Option<String>, Option<String>) {
    let Some(payload) = token.split('.').nth(1) else {
        return (None, None);
    };
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return (None, None);
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return (None, None);
    };
    let email = claims
        .get("email")
        .and_then(|v| v.as_str())
        .map(String::from);
    let sub = claims
        .get("sub")
        .or_else(|| claims.get("account_id"))
        .and_then(|v| v.as_str())
        .map(|s| format!("acc_{s}"));
    (email, sub)
}
