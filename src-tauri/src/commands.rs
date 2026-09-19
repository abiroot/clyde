//! Tauri commands — the typed bridge the React UI calls into.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;

use serde::Serialize;
use tauri::State;

use crate::engine::SharedCore;
use crate::model::{now_ms, Account, AppSnapshot, Credential};
use crate::{chrome_link, claude_sync, import_claude, oauth, open_chrome};

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

/// Which account the Claude-in-Chrome extension is signed into. Read on demand
/// rather than folded into [`AppSnapshot`]: it touches the browser's on-disk
/// storage, and the answer only matters when the user is looking at it.
#[tauri::command]
pub fn get_chrome_link(core: State<SharedCore>) -> chrome_link::ChromeLink {
    core.chrome_link()
}

/// State of the account-independent browser tools (Open Claude in Chrome).
/// Reads browser preference files and probes a socket, so it's on demand too.
#[tauri::command]
pub fn get_open_chrome() -> open_chrome::OpenChromeStatus {
    open_chrome::status()
}

/// Download (if needed) and wire up Open Claude in Chrome. Runs git/npm, so it
/// goes off the main thread.
#[tauri::command]
pub async fn setup_open_chrome() -> CmdResult<open_chrome::OpenChromeStatus> {
    tauri::async_runtime::spawn_blocking(open_chrome::setup)
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn remove_open_chrome() -> CmdResult<open_chrome::OpenChromeStatus> {
    tauri::async_runtime::spawn_blocking(open_chrome::remove)
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn set_builtin_chrome(enabled: bool) -> CmdResult<open_chrome::OpenChromeStatus> {
    open_chrome::set_builtin(enabled).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn reveal_open_chrome_extension() -> CmdResult<()> {
    open_chrome::reveal_extension().map_err(err)
}

#[tauri::command]
pub fn open_chrome_extensions_page() -> CmdResult<()> {
    open_chrome::open_extensions_page().map_err(err)
}

/// Put text on the macOS clipboard. `pbcopy` is dependable where the webview's
/// own clipboard API may be unavailable.
#[tauri::command]
pub fn copy_text(text: String) -> CmdResult<()> {
    use std::io::Write;
    let mut child = Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(err)?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(text.as_bytes())
        .map_err(err)?;
    child.wait().map_err(err)?;
    Ok(())
}

#[tauri::command]
pub fn get_settings(core: State<SharedCore>) -> crate::settings::Settings {
    core.settings()
}

/// Save settings and apply the parts that live outside the engine (shortcut).
#[tauri::command]
pub fn set_settings(
    app: tauri::AppHandle,
    core: State<SharedCore>,
    settings: crate::settings::Settings,
) -> CmdResult<crate::settings::Settings> {
    let saved = core.set_settings(settings).map_err(err)?;
    crate::apply_shortcut(&app, &saved.shortcut);
    Ok(saved)
}

/// Usage history for one account over the last `hours`.
#[tauri::command]
pub fn get_history(
    core: State<SharedCore>,
    account_id: String,
    hours: i64,
) -> Vec<crate::history::Point> {
    core.history(&account_id, now_ms() - hours.max(1) * 3_600_000)
}

#[tauri::command]
pub async fn list_sessions() -> Vec<crate::sessions::Session> {
    tauri::async_runtime::spawn_blocking(crate::sessions::list)
        .await
        .unwrap_or_default()
}

#[tauri::command]
pub fn test_notification(core: State<SharedCore>) {
    core.notify(
        "Clyde alerts are on",
        "You'll hear from Clyde like this when a limit crosses your thresholds.",
    );
}

#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> CmdResult<bool> {
    use tauri_plugin_autostart::ManagerExt;
    let m = app.autolaunch();
    if enabled { m.enable() } else { m.disable() }.map_err(err)?;
    Ok(m.is_enabled().unwrap_or(enabled))
}

/// What `set_active_account` returns: the fresh snapshot, plus how many
/// `claude` sessions were running at switch time so the UI can tell the user
/// what the switch means for them (new credential within ~30 s; a connected
/// Chrome browser bridge drops and needs `/chrome` → Reconnect, or a restart).
#[derive(Serialize)]
pub struct SwitchOutcome {
    pub snapshot: AppSnapshot,
    pub running_sessions: u32,
}

/// Make `id` the account Claude Code uses, by writing its OAuth into Claude
/// Code's own credential store. New `claude` runs use it immediately; running
/// sessions pick it up within ~30 seconds.
#[tauri::command]
pub async fn set_active_account(
    core: State<'_, SharedCore>,
    id: String,
) -> CmdResult<SwitchOutcome> {
    tracing::info!("set_active_account: {id}");
    core.set_active(&id).await.map_err(|e| {
        tracing::error!("set_active({id}) failed: {e:#}");
        err(e)
    })?;
    tracing::info!("set_active_account: {id} ok");
    Ok(SwitchOutcome {
        snapshot: core.snapshot(),
        running_sessions: claude_sync::running_claude_sessions(),
    })
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
    // Read its usage now rather than at the next 2-minute poll.
    spawn_usage_poll(core.inner().clone());
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
    spawn_usage_poll(core.inner().clone());
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
        oauth_account: profile.as_ref().map(|p| p.merge_into_oauth_account(None)),
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
