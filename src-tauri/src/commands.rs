//! Tauri commands — the typed bridge the React UI calls into.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;

use serde::Serialize;
use tauri::State;

use crate::engine::SharedCore;
use crate::model::{now_ms, Account, AppSnapshot, Credential};
use crate::{import_claude, oauth};

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

/// Make `id` the account Claude Code uses, by writing its OAuth into Claude
/// Code's own credential store. Takes effect on the next `claude` run.
#[tauri::command]
pub async fn set_active_account(core: State<'_, SharedCore>, id: String) -> CmdResult<AppSnapshot> {
    tracing::info!("set_active_account: {id}");
    core.set_active(&id).await.map_err(|e| {
        tracing::error!("set_active({id}) failed: {e:#}");
        err(e)
    })?;
    tracing::info!("set_active_account: {id} ok");
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

    let account = account_from_token(&core.http, credential, &label).await;
    core.add_account(account).map_err(err)?;
    Ok(core.snapshot())
}

/// Import an already-authenticated session by pasting its token JSON
/// (`{ accessToken, refreshToken, expiresAt, ... }`). A no-OAuth fallback.
#[tauri::command]
pub async fn import_token(
    core: State<'_, SharedCore>,
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
    let account = account_from_token(&core.http, credential, &label).await;
    core.add_account(account).map_err(err)?;
    Ok(core.snapshot())
}

/// Find Claude Code accounts already on this machine (keychain + config dirs).
#[tauri::command]
pub fn discover_claude_accounts() -> CmdResult<Vec<import_claude::Discovered>> {
    import_claude::discover().map_err(err)
}

/// Add a brand-new account by delegating to Claude Code's own (maintained)
/// login: create an isolated config dir and open a terminal running `claude`
/// there. The user signs in via `/login`, then Clyde imports from that dir.
/// Returns the config dir to pass to `import_claude_accounts`.
#[tauri::command]
pub fn start_claude_login() -> CmdResult<String> {
    let home = std::env::var("HOME").map_err(err)?;
    let dir = format!("{home}/.claude-clyde-{}", now_ms());
    std::fs::create_dir_all(&dir).map_err(err)?;

    let shell_cmd = format!("CLAUDE_CONFIG_DIR='{dir}' claude");
    let apple = format!(
        "tell application \"Terminal\" to do script \"{}\"",
        shell_cmd.replace('\\', "\\\\").replace('"', "\\\"")
    );
    Command::new("osascript")
        .arg("-e")
        .arg("tell application \"Terminal\" to activate")
        .arg("-e")
        .arg(&apple)
        .spawn()
        .map_err(|e| format!("couldn't open Terminal to run claude: {e}"))?;

    Ok(dir)
}

/// Import the chosen discovered accounts (identified by their config dir).
#[tauri::command]
pub fn import_claude_accounts(
    core: State<SharedCore>,
    config_dirs: Vec<String>,
) -> CmdResult<AppSnapshot> {
    for dir in &config_dirs {
        let account = import_claude::import_account(dir).map_err(err)?;
        core.add_account(account).map_err(err)?;
    }
    spawn_usage_poll(core.inner().clone());
    Ok(core.snapshot())
}

/// Kick an immediate usage refresh in the background (after adding accounts).
fn spawn_usage_poll(core: SharedCore) {
    tauri::async_runtime::spawn(async move { core.poll_usage().await });
}

// ---- helpers --------------------------------------------------------------

/// Build an [`Account`] from a freshly-obtained credential (browser OAuth or
/// pasted token). Claude's access tokens are opaque, so the identity (email,
/// plan) comes from `GET /api/oauth/profile`. The id is derived from the email
/// so this merges with the same account if it's later discovered on disk.
async fn account_from_token(
    http: &reqwest::Client,
    credential: Credential,
    label: &str,
) -> Account {
    let profile = oauth::fetch_profile(http, &credential.access_token)
        .await
        .map_err(|e| tracing::warn!("profile lookup failed; account will lack identity: {e:#}"))
        .ok();

    let email = profile.as_ref().and_then(|p| p.email.clone());
    let subscription_raw = profile.as_ref().and_then(|p| p.subscription_raw.clone());
    let rate_limit_tier = profile.as_ref().and_then(|p| p.rate_limit_tier.clone());
    let full_name = profile.as_ref().and_then(|p| p.full_name.clone());

    let id = match &email {
        Some(e) => import_claude::account_id_for_email(e),
        None => gen_id("acc"),
    };
    let label = if label.trim().is_empty() {
        email
            .clone()
            .or(full_name)
            .unwrap_or_else(|| "Claude account".to_string())
    } else {
        label.to_string()
    };
    let subscription_type = rate_limit_tier
        .as_deref()
        .map(import_claude::plan_label)
        .or_else(|| subscription_raw.clone().map(|s| title_case(&s)));

    Account {
        id,
        label,
        oauth_account: email
            .as_ref()
            .map(|e| serde_json::json!({ "emailAddress": e })),
        email,
        subscription_type,
        subscription_raw,
        rate_limit_tier,
        credential,
        source_config_dir: None,
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn gen_id(prefix: &str) -> String {
    use rand::Rng;
    let n: u64 = rand::thread_rng().gen();
    format!("{prefix}_{:x}{:x}", now_ms(), n)
}
