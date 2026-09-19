//! The Clyde engine: shared, in-memory source of truth for the Tauri command
//! layer. Holds accounts, the active selection, live usage, and the
//! token-refresh machinery. Switching an account writes its OAuth straight into
//! Claude Code's own credential store via [`crate::claude_sync`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Context, Result};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::history::{History, Point};
use crate::model::*;
use crate::settings::Settings;
use crate::{alerts, chrome_link, claude_sync, import_claude, oauth, settings, vault};

/// The tray icon's id (see `lib.rs`), for the menubar readout.
pub const TRAY_ID: &str = "clyde-tray";

/// Refresh an access token this many ms before it actually expires.
const REFRESH_SKEW_MS: i64 = 60_000;

/// Event name the frontend subscribes to for live state pushes.
pub const UPDATE_EVENT: &str = "clyde://update";

pub type SharedCore = Arc<Core>;

pub struct Core {
    pub http: reqwest::Client,
    state: RwLock<CoreState>,
    refresh_lock: tokio::sync::Mutex<()>,
    app: RwLock<Option<AppHandle>>,
    /// App data directory (settings, history); known once the app is attached.
    data_dir: RwLock<Option<PathBuf>>,
    prefs: RwLock<Settings>,
    history: RwLock<History>,
}

struct CoreState {
    accounts: Vec<Account>,
    usage: HashMap<String, UsageSnapshot>,
    usage_errors: HashMap<String, String>,
    active_id: Option<String>,
}

impl Core {
    pub fn new() -> Result<SharedCore> {
        let accounts = vault::load_accounts()?;
        let active_id = accounts.first().map(|a| a.id.clone());
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .map_err(|e| anyhow!("building http client: {e}"))?;

        Ok(Arc::new(Core {
            http,
            refresh_lock: tokio::sync::Mutex::new(()),
            app: RwLock::new(None),
            data_dir: RwLock::new(None),
            prefs: RwLock::new(Settings::default()),
            history: RwLock::new(History::default()),
            state: RwLock::new(CoreState {
                accounts,
                usage: HashMap::new(),
                usage_errors: HashMap::new(),
                active_id,
            }),
        }))
    }

    pub fn attach_app(&self, app: AppHandle) {
        if let Ok(dir) = app.path().app_data_dir() {
            *self.prefs.write().unwrap() = settings::load(&dir);
            *self.history.write().unwrap() = History::open(&dir, now_ms());
            *self.data_dir.write().unwrap() = Some(dir);
        }
        *self.app.write().unwrap() = Some(app);
    }

    // ---- settings ---------------------------------------------------------

    pub fn settings(&self) -> Settings {
        self.prefs.read().unwrap().clone()
    }

    pub fn set_settings(&self, new: Settings) -> Result<Settings> {
        let new = new.normalized();
        if let Some(dir) = self.data_dir.read().unwrap().as_ref() {
            settings::save(dir, &new)?;
        }
        *self.prefs.write().unwrap() = new.clone();
        self.update_tray_readout();
        Ok(new)
    }

    /// An account's usage history since `since` (ms).
    pub fn history(&self, account_id: &str, since: i64) -> Vec<Point> {
        self.history.read().unwrap().series(account_id, since)
    }

    /// Put the active account's tightest limit next to the menubar icon.
    pub fn update_tray_readout(&self) {
        let Some(app) = self.app.read().unwrap().clone() else {
            return;
        };
        let Some(tray) = app.tray_by_id(TRAY_ID) else {
            return;
        };
        let title = if self.prefs.read().unwrap().menubar_readout {
            let s = self.state.read().unwrap();
            s.active_id
                .as_ref()
                .and_then(|id| s.usage.get(id))
                .and_then(tightest)
                .map(|p| format!("{}%", p.round()))
        } else {
            None
        };
        let _ = tray.set_title(title);
    }

    /// Send a test notification (Settings → Alerts).
    pub fn notify(&self, title: &str, body: &str) {
        if let Some(app) = self.app.read().unwrap().as_ref() {
            let _ = app.notification().builder().title(title).body(body).show();
        }
    }

    // ---- snapshot / UI ----------------------------------------------------

    pub fn snapshot(&self) -> AppSnapshot {
        let s = self.state.read().unwrap();
        let active_id = s.active_id.clone();
        let active_email = active_id
            .as_ref()
            .and_then(|id| s.accounts.iter().find(|a| &a.id == id))
            .and_then(|a| a.email.clone());

        let accounts = s
            .accounts
            .iter()
            .map(|a| AccountView {
                id: a.id.clone(),
                label: a.label.clone(),
                email: a.email.clone(),
                subscription_type: a.subscription_type.clone(),
                usage: s.usage.get(&a.id).cloned().unwrap_or_default(),
                is_active: Some(&a.id) == active_id.as_ref(),
                usage_error: s.usage_errors.get(&a.id).cloned(),
                forecasts: self.history.read().unwrap().forecasts(&a.id, now_ms()),
            })
            .collect();

        AppSnapshot {
            accounts,
            active_id,
            active_email,
        }
    }

    /// Push the current snapshot to the UI.
    pub fn emit(&self) {
        if let Some(app) = self.app.read().unwrap().as_ref() {
            let _ = app.emit(UPDATE_EVENT, self.snapshot());
        }
    }

    // ---- account management ----------------------------------------------

    pub fn add_account(&self, account: Account) -> Result<()> {
        {
            let mut s = self.state.write().unwrap();
            // A re-added account has a fresh login: whatever went wrong with the
            // old one (e.g. "Couldn't refresh the login") no longer applies.
            s.usage_errors.remove(&account.id);
            if let Some(existing) = s.accounts.iter_mut().find(|a| a.id == account.id) {
                *existing = account;
            } else {
                if s.active_id.is_none() {
                    s.active_id = Some(account.id.clone());
                }
                s.accounts.push(account);
            }
            vault::save_accounts(&s.accounts)?;
        }
        self.emit();
        Ok(())
    }

    pub fn remove_account(&self, id: &str) -> Result<()> {
        {
            let mut s = self.state.write().unwrap();
            s.accounts.retain(|a| a.id != id);
            s.usage.remove(id);
            s.usage_errors.remove(id);
            if s.active_id.as_deref() == Some(id) {
                s.active_id = s.accounts.first().map(|a| a.id.clone());
            }
            vault::save_accounts(&s.accounts)?;
        }
        self.emit();
        Ok(())
    }

    pub fn rename_account(&self, id: &str, label: &str) -> Result<()> {
        {
            let mut s = self.state.write().unwrap();
            if let Some(a) = s.accounts.iter_mut().find(|a| a.id == id) {
                a.label = label.to_string();
            }
            vault::save_accounts(&s.accounts)?;
        }
        self.emit();
        Ok(())
    }

    #[allow(dead_code)] // used by the tray/menubar status
    pub fn has_accounts(&self) -> bool {
        !self.state.read().unwrap().accounts.is_empty()
    }

    // ---- active account ---------------------------------------------------

    /// Make `id` the account Claude Code uses: refresh its token if stale, then
    /// write it into Claude Code's keychain + `.claude.json`. Takes effect for
    /// the next `claude` run.
    pub async fn set_active(&self, id: &str) -> Result<()> {
        // Pull the live keychain credential back into whichever stored account
        // it belongs to — Claude Code may have rotated it since we wrote it,
        // and the rotation would be lost once we overwrite the slot.
        self.adopt_active_slot();

        // If the user ran `claude` against this account's source config dir,
        // that dir holds a newer credential than ours; adopt it.
        self.resync_credential_from_source(id);

        // Hand Claude Code a fresh token. A transient failure (network blip)
        // shouldn't block switching while the stored token is still valid —
        // `claude` refreshes on its own at startup.
        if let Err(e) = self.valid_bearer(id).await {
            let still_valid = self
                .credential(id)
                .is_some_and(|c| !c.is_stale(now_ms(), 0));
            if still_valid {
                tracing::warn!(
                    "pre-switch refresh failed; switching with the current token: {e:#}"
                );
            } else {
                return Err(e.context(
                    "couldn't refresh this account's token — its saved login may have expired; remove the account and add it again",
                ));
            }
        }

        let account = self
            .account(id)
            .ok_or_else(|| anyhow!("unknown account {id}"))?;
        claude_sync::activate(&account)
            .context("writing the account into Claude Code's keychain")?;
        {
            let mut s = self.state.write().unwrap();
            s.active_id = Some(id.to_string());
        }
        self.emit();
        Ok(())
    }

    /// Which account the Claude-in-Chrome extension is signed into, and whether
    /// that is the account currently active. Switching in Clyde leaves the
    /// browser where it was, so `/chrome` stops working until the two agree —
    /// see [`crate::chrome_link`] for why Clyde can only report this, not fix it
    /// silently.
    pub fn chrome_link(&self) -> chrome_link::ChromeLink {
        let (known, active_uuid) = {
            let s = self.state.read().unwrap();
            let uuid_of = |a: &Account| {
                a.oauth_account
                    .as_ref()
                    .and_then(|v| v.get("accountUuid"))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            };
            let known: Vec<(String, String, Option<String>)> = s
                .accounts
                .iter()
                .filter_map(|a| Some((uuid_of(a)?, a.id.clone(), a.email.clone())))
                .collect();
            let active_uuid = s
                .active_id
                .as_ref()
                .and_then(|id| s.accounts.iter().find(|a| &a.id == id))
                .and_then(uuid_of)
                // Before the backfill has run, fall back to whatever Claude
                // Code itself recorded for the active identity.
                .or_else(claude_sync::read_active_identity_uuid);
            (known, active_uuid)
        };
        chrome_link::detect(&known, active_uuid.as_deref())
    }

    /// Reconcile Claude Code's shared keychain slot with our vault, in both
    /// directions. Works out which stored account the slot's credential belongs
    /// to (token lineage first, then the identity in `~/.claude/.claude.json`)
    /// and then adopts whichever side holds the newer token generation: the
    /// keychain's, when a running `claude` rotated it on its own; the vault's,
    /// when a past Clyde refresh never made it back into the keychain (leaving
    /// `claude` a consumed refresh token — a forced logout on its next run).
    /// Matching is identity-checked so credentials can never cross between
    /// accounts. Returns the matched account's id.
    fn adopt_active_slot(&self) -> Option<String> {
        let live = claude_sync::read_active_credential()?;
        let email = claude_sync::read_active_identity_email();

        let (id, repair) = {
            let mut s = self.state.write().unwrap();
            let idx = s
                .accounts
                .iter()
                .position(|a| {
                    (!live.refresh_token.is_empty()
                        && a.credential.refresh_token == live.refresh_token)
                        || a.credential.access_token == live.access_token
                })
                .or_else(|| {
                    let email = email.as_deref()?;
                    s.accounts
                        .iter()
                        .position(|a| a.email.as_deref() == Some(email))
                })?;

            let mut repair = None;
            if live.is_newer_than(&s.accounts[idx].credential) {
                let who = s.accounts[idx]
                    .email
                    .clone()
                    .unwrap_or_else(|| s.accounts[idx].id.clone());
                s.accounts[idx].credential = live.clone();
                let _ = vault::save_accounts(&s.accounts);
                tracing::info!("adopted a rotated credential from the keychain for {who}");
            } else if s.accounts[idx].credential.is_newer_than(&live) {
                repair = Some(s.accounts[idx].credential.clone());
            }
            (s.accounts[idx].id.clone(), repair)
        };

        if let Some(cred) = repair {
            match claude_sync::write_active_credential(&cred, &live) {
                Ok(true) => {
                    tracing::info!("repaired the keychain slot with our newer credential")
                }
                Ok(false) => {}
                Err(e) => tracing::warn!("couldn't repair the keychain slot: {e:#}"),
            }
        }
        Some(id)
    }

    /// Reconcile `active_id` with whichever account Claude Code is actually set
    /// to right now. Also recovers newer credentials from source config dirs
    /// and the shared keychain slot.
    pub fn detect_active(&self) {
        self.sync_all_credentials_from_sources();

        // No "first account" fallback: claiming an account is active while the
        // keychain actually holds someone else's login is how tokens cross.
        let resolved = self.adopt_active_slot();
        {
            let mut s = self.state.write().unwrap();
            s.active_id = resolved;
        }
        self.repair_active_identity();
        self.emit();
    }

    /// Heal a stale identity block in Claude Code's config, so it names whoever
    /// actually owns the keychain slot. Cheap and idempotent — a read, and a
    /// write only when the two disagree.
    fn repair_active_identity(&self) {
        let active = {
            let s = self.state.read().unwrap();
            s.active_id
                .as_ref()
                .and_then(|id| s.accounts.iter().find(|a| &a.id == id))
                .cloned()
        };
        let Some(account) = active else { return };
        match claude_sync::repair_identity(&account) {
            Ok(true) => tracing::info!(
                "refreshed Claude Code's identity block for {}",
                account.email.as_deref().unwrap_or(&account.id)
            ),
            Ok(false) => {}
            Err(e) => tracing::warn!("couldn't refresh the identity block: {e:#}"),
        }
    }

    /// On startup, sync all accounts' credentials from their source config dirs.
    /// Claude Code keeps those tokens fresh, so this ensures Clyde has valid tokens.
    fn sync_all_credentials_from_sources(&self) {
        let ids: Vec<String> = {
            let s = self.state.read().unwrap();
            s.accounts.iter().map(|a| a.id.clone()).collect()
        };
        for id in ids {
            self.resync_credential_from_source(&id);
        }
    }

    /// Remove leftover `~/.claude-clyde-*` profiles from the "New account" flow
    /// that no stored account still references — abandoned or already-imported
    /// sign-in attempts. Only ever touches dirs Clyde itself created.
    pub fn cleanup_orphan_login_dirs(&self) {
        let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) else {
            return;
        };
        let in_use: std::collections::HashSet<String> = {
            let s = self.state.read().unwrap();
            s.accounts
                .iter()
                .filter_map(|a| a.source_config_dir.clone())
                .collect()
        };
        let Ok(entries) = std::fs::read_dir(&home) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".claude-clyde-")
                && path.is_dir()
                && !in_use.contains(&path.to_string_lossy().to_string())
            {
                if let Err(e) = std::fs::remove_dir_all(&path) {
                    tracing::debug!("couldn't remove stale login dir {name}: {e}");
                } else {
                    tracing::info!("removed stale sign-in profile {name}");
                }
            }
        }
    }

    fn account(&self, id: &str) -> Option<Account> {
        self.state
            .read()
            .unwrap()
            .accounts
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    /// Refresh an account's stored credential from a live Claude Code config dir
    /// — the one it was imported from, or any dir whose account email matches.
    /// Claude Code keeps those tokens fresh, so this recovers the common case
    /// where our stored snapshot has expired. Best-effort: silent on failure.
    fn resync_credential_from_source(&self, id: &str) {
        let (email, source) = {
            let s = self.state.read().unwrap();
            match s.accounts.iter().find(|a| a.id == id) {
                Some(a) => (a.email.clone(), a.source_config_dir.clone()),
                None => return,
            }
        };

        // Try the recorded import dir first, then any config dir on the system
        // whose account email matches this one.
        let mut dirs: Vec<String> = source.into_iter().collect();
        if email.is_some() {
            if let Ok(found) = import_claude::discover() {
                for d in found {
                    if d.email == email && !dirs.contains(&d.config_dir) {
                        dirs.push(d.config_dir);
                    }
                }
            }
        }

        for dir in dirs {
            // Never resync from the default `~/.claude`: its keychain is the
            // shared slot Clyde overwrites on every switch, so it holds whichever
            // account is *currently active* — reading it here would cross this
            // account's stored token with another's. The default slot is handled
            // by `adopt_active_slot`, which matches the credential to its owner.
            if import_claude::is_default_config_dir(&dir) {
                continue;
            }
            let Ok(fresh) = import_claude::import_account(&dir) else {
                continue;
            };
            let mut s = self.state.write().unwrap();
            let Some(a) = s.accounts.iter_mut().find(|a| a.id == id) else {
                return;
            };
            // Source keychains go stale the moment Clyde refreshes this account:
            // a refresh *consumes* the old refresh token, so blindly adopting the
            // source copy would store a dead token — the resulting invalid_grant
            // is a forced logout. Only adopt a strictly newer generation, i.e.
            // the user actually ran `claude` in that dir since our last refresh.
            let mut changed = false;
            if fresh.credential.is_newer_than(&a.credential) {
                a.credential = fresh.credential;
                changed = true;
            }
            if fresh.oauth_account.is_some() && a.oauth_account.is_none() {
                a.oauth_account = fresh.oauth_account;
                changed = true;
            }
            if a.source_config_dir.is_none() {
                a.source_config_dir = Some(dir.clone());
                changed = true;
            }
            if changed {
                let _ = vault::save_accounts(&s.accounts);
            }
        }
    }

    // ---- usage + tokens ---------------------------------------------------

    pub fn record_usage(&self, account_id: &str, snap: UsageSnapshot) {
        let (prev, name, is_active) = {
            let mut s = self.state.write().unwrap();
            s.usage_errors.remove(account_id);
            let prev = s.usage.insert(account_id.to_string(), snap.clone());
            let name = s
                .accounts
                .iter()
                .find(|a| a.id == account_id)
                .map(|a| a.email.clone().unwrap_or_else(|| a.label.clone()))
                .unwrap_or_default();
            (prev, name, s.active_id.as_deref() == Some(account_id))
        };

        self.history
            .write()
            .unwrap()
            .push(Point::from_snapshot(account_id, &snap));

        let prefs = self.settings();
        if is_active || prefs.alert_all_accounts {
            for a in alerts::evaluate(&name, prev.as_ref(), &snap, &prefs, now_ms() / 1000) {
                self.notify(&a.title, &a.body);
            }
        }

        self.update_tray_readout();
        self.emit();
    }

    /// Remember why an account's usage couldn't be read.
    fn record_usage_error(&self, account_id: &str, error: String) {
        self.state
            .write()
            .unwrap()
            .usage_errors
            .insert(account_id.to_string(), error);
        self.emit();
    }

    /// Probe every account's live usage with a tiny request, so the gauges fill
    /// even when no traffic is flowing.
    pub async fn poll_usage(&self) {
        // Keep our copy of the active slot current with claude's own rotations,
        // and follow along if the user switched accounts via `claude /login`.
        if let Some(id) = self.adopt_active_slot() {
            let changed = {
                let mut s = self.state.write().unwrap();
                if s.active_id.as_deref() != Some(id.as_str()) {
                    s.active_id = Some(id);
                    true
                } else {
                    false
                }
            };
            if changed {
                self.emit();
            }
        }

        let ids: Vec<String> = {
            let s = self.state.read().unwrap();
            s.accounts.iter().map(|a| a.id.clone()).collect()
        };
        for id in ids {
            self.backfill_identity(&id).await;
            if let Err(e) = self.fetch_account_usage(&id).await {
                tracing::debug!("usage fetch failed for {id}: {e:#}");
                self.record_usage_error(&id, short_error(&e));
            }
        }

        // The backfill may have just learned the active account's uuid, which
        // startup didn't have to write into Claude Code's config.
        self.repair_active_identity();
    }

    /// Fill in `oauthAccount.accountUuid` for accounts stored before Clyde
    /// captured it (imports and token pastes only ever synthesized an email).
    /// One profile lookup per account, once — after which this is a no-op.
    ///
    /// The uuid is what Claude Code writes into its own config and what the
    /// Chrome extension keys its bridge connection on, so without it Clyde can
    /// neither restore a complete identity nor tell which account the browser is
    /// signed into.
    async fn backfill_identity(&self, account_id: &str) {
        let existing = {
            let s = self.state.read().unwrap();
            let Some(a) = s.accounts.iter().find(|a| a.id == account_id) else {
                return;
            };
            let has_uuid = a
                .oauth_account
                .as_ref()
                .and_then(|v| v.get("accountUuid"))
                .is_some();
            if has_uuid {
                return;
            }
            a.oauth_account.clone()
        };

        let Ok(bearer) = self.valid_bearer(account_id).await else {
            return;
        };
        let profile = match oauth::fetch_profile(&self.http, &bearer).await {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!("identity backfill failed for {account_id}: {e:#}");
                return;
            }
        };
        if profile.account_uuid.is_none() {
            return;
        }
        let merged = profile.merge_into_oauth_account(existing.as_ref());

        let mut s = self.state.write().unwrap();
        if let Some(a) = s.accounts.iter_mut().find(|a| a.id == account_id) {
            a.oauth_account = Some(merged);
            let _ = vault::save_accounts(&s.accounts);
            tracing::info!("cached accountUuid for {account_id}");
        }
    }

    /// Read an account's live usage from `GET /api/oauth/usage` — the same
    /// endpoint Claude Code itself uses for its status line. Unlike a probe
    /// request this consumes no quota and returns every rolling window directly.
    async fn fetch_account_usage(&self, account_id: &str) -> Result<()> {
        let bearer = self.valid_bearer(account_id).await?;
        let url = format!("{}/api/oauth/usage", upstream().trim_end_matches('/'));
        let resp = self
            .http
            .get(url)
            .header("authorization", format!("Bearer {bearer}"))
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(anyhow!(
                "usage endpoint returned {}",
                resp.status().as_u16()
            ));
        }
        let body: serde_json::Value = resp.json().await?;
        if let Some(snap) = crate::usage::parse(&body) {
            self.record_usage(account_id, snap);
        }
        Ok(())
    }

    /// Return a valid (fresh) access token for `account_id`, refreshing if it's
    /// stale and persisting the rotated credential. When the account owns Claude
    /// Code's keychain slot, the rotation is pushed there too — refresh tokens
    /// are single-use, so leaving the consumed one in the keychain would log
    /// the next `claude` run out.
    pub async fn valid_bearer(&self, account_id: &str) -> Result<String> {
        // Fast path: token still good.
        if let Some(cred) = self.credential(account_id) {
            if !cred.is_stale(now_ms(), REFRESH_SKEW_MS) {
                return Ok(cred.access_token);
            }
        } else {
            return Err(anyhow!("unknown account {account_id}"));
        }

        // Slow path: serialize refreshes so concurrent requests don't stampede.
        let _guard = self.refresh_lock.lock().await;
        let Some(cred) = self.credential(account_id) else {
            return Err(anyhow!("unknown account {account_id}"));
        };
        if !cred.is_stale(now_ms(), REFRESH_SKEW_MS) {
            return Ok(cred.access_token); // someone else refreshed
        }

        let is_active = self.is_active(account_id);
        let mut prev = cred;
        let refreshed = match oauth::refresh(&self.http, &prev.refresh_token, &prev.scopes).await {
            Ok(r) => r,
            Err(e) => {
                // A running `claude` may have rotated the active slot after we
                // last synced, consuming the refresh token we just tried. Re-read
                // the keychain and go again with the live generation.
                let live = is_active
                    .then(claude_sync::read_active_credential)
                    .flatten()
                    .filter(|l| {
                        !l.refresh_token.is_empty() && l.refresh_token != prev.refresh_token
                    });
                let Some(live) = live else { return Err(e) };
                tracing::info!(
                    "refresh for {account_id} failed but the keychain holds a newer generation; retrying with it"
                );
                if !live.is_stale(now_ms(), REFRESH_SKEW_MS) {
                    let token = live.access_token.clone();
                    self.update_credential(account_id, live)?;
                    return Ok(token);
                }
                let r = oauth::refresh(&self.http, &live.refresh_token, &live.scopes).await?;
                prev = live;
                r
            }
        };

        let token = refreshed.access_token.clone();
        self.update_credential(account_id, refreshed.clone())?;
        if is_active {
            match claude_sync::write_active_credential(&refreshed, &prev) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!("keychain slot no longer holds {account_id}; left it alone")
                }
                Err(e) => tracing::warn!("couldn't push rotated credential to the keychain: {e:#}"),
            }
        }
        Ok(token)
    }

    fn is_active(&self, id: &str) -> bool {
        self.state.read().unwrap().active_id.as_deref() == Some(id)
    }

    fn credential(&self, account_id: &str) -> Option<Credential> {
        let s = self.state.read().unwrap();
        s.accounts
            .iter()
            .find(|a| a.id == account_id)
            .map(|a| a.credential.clone())
    }

    fn update_credential(&self, account_id: &str, cred: Credential) -> Result<()> {
        let mut s = self.state.write().unwrap();
        if let Some(a) = s.accounts.iter_mut().find(|a| a.id == account_id) {
            a.credential = cred;
        }
        vault::save_accounts(&s.accounts)
    }
}

fn upstream() -> String {
    std::env::var("CLYDE_UPSTREAM").unwrap_or_else(|_| "https://api.anthropic.com".to_string())
}

/// The highest percentage across an account's limits — what decides whether
/// it can take more work.
pub fn tightest(u: &UsageSnapshot) -> Option<f64> {
    u.five_hour_utilization
        .into_iter()
        .chain(u.seven_day_utilization)
        .chain(u.scoped_limits.iter().map(|l| l.percent))
        .reduce(f64::max)
}

/// A one-line, user-facing reason for a failed usage read.
fn short_error(e: &anyhow::Error) -> String {
    let msg = format!("{e:#}");
    if msg.contains("returned 429") {
        "Anthropic is rate-limiting usage checks; retrying".into()
    } else if msg.contains("returned 401") || msg.contains("returned 403") {
        "Signed out — add this account again".into()
    } else if msg.contains("refresh") {
        "Couldn't refresh the login".into()
    } else {
        "Couldn't read usage".into()
    }
}
