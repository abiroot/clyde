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

use crate::autoswitch::{self, Candidate, Decision, Reason};
use crate::history::{History, Point};
use crate::model::*;
use crate::provider::{ModelInfo, Provider};
use crate::routing::{self, ProviderRoute, Target};
use crate::settings::Settings;
use crate::updater::UpdateInfo;
use crate::{alerts, chrome_link, claude_sync, import_claude, oauth, settings, vault};

/// The tray icon's id (see `lib.rs`), for the menubar readout.
pub const TRAY_ID: &str = "clyde-tray";

/// Refresh an access token this many ms before it actually expires.
const REFRESH_SKEW_MS: i64 = 60_000;

/// Event name the frontend subscribes to for live state pushes.
pub const UPDATE_EVENT: &str = "clyde://update";

/// Minimum gap between two automatic switch-backs (ms). Switching away from a
/// used-up account is never delayed.
const SWITCH_BACK_GAP_MS: i64 = 3 * 60 * 1000;

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
    /// A newer Clyde release, once the updater has found one.
    update: RwLock<Option<UpdateInfo>>,
    /// The localhost provider relay is listening.
    relay_up: std::sync::atomic::AtomicBool,
    /// Each provider's models, as last read (or the built-in fallback).
    catalogs: RwLock<HashMap<Provider, Vec<ModelInfo>>>,
    /// When each provider's model list was last read (ms).
    catalog_read_at: RwLock<HashMap<Provider, i64>>,
    /// Routing bookkeeping: chosen models, pins, what to restore.
    routing: RwLock<routing::Prefs>,
    /// `settings.json`'s last seen modification time, for the `/model` watch.
    settings_seen: std::sync::Mutex<Option<std::time::SystemTime>>,
}

struct CoreState {
    accounts: Vec<Account>,
    usage: HashMap<String, UsageSnapshot>,
    usage_errors: HashMap<String, String>,
    active_id: Option<String>,
    /// The user chose the active account themselves (in Clyde or via
    /// `claude /login`): auto-switch won't switch *back* from it until it trips.
    manual_hold: bool,
    /// The most recent automatic switch, for the UI.
    last_switch: Option<SwitchEvent>,
    /// Already told the user every account is used up (once per episode).
    exhausted_notified: bool,
    /// Keys of the connected providers. They aren't stored with the Claude
    /// accounts: there's no OAuth to refresh, only the key.
    keys: HashMap<Provider, String>,
}

/// One automatic switch.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SwitchEvent {
    pub from: String,
    pub to: String,
    /// e.g. "Session ≥ 90%", or "back to the preferred account".
    pub reason: String,
    /// Unix ms.
    pub at: i64,
}

impl Core {
    pub fn new() -> Result<SharedCore> {
        // A Keychain read can fail right after an update (macOS has to approve
        // the new binary, and can't ask while the old one is relaunching it).
        // Start anyway with nothing loaded — saving stays blocked until the
        // real list arrives — and keep retrying in the background.
        let accounts = match vault::load_accounts() {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!("couldn't read saved accounts yet, will retry: {e:#}");
                Vec::new()
            }
        };
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
            update: RwLock::new(None),
            relay_up: std::sync::atomic::AtomicBool::new(false),
            catalogs: RwLock::new(HashMap::new()),
            catalog_read_at: RwLock::new(HashMap::new()),
            routing: RwLock::new(routing::Prefs::default()),
            settings_seen: std::sync::Mutex::new(None),
            state: RwLock::new(CoreState {
                accounts,
                usage: HashMap::new(),
                usage_errors: HashMap::new(),
                active_id,
                manual_hold: false,
                last_switch: None,
                exhausted_notified: false,
                // Loaded off the startup path (see `load_provider_keys`): a
                // Keychain prompt must never hold the app up.
                keys: HashMap::new(),
            }),
        }))
    }

    pub fn attach_app(&self, app: AppHandle) {
        if let Ok(dir) = app.path().app_data_dir() {
            *self.prefs.write().unwrap() = settings::load(&dir);
            *self.history.write().unwrap() = History::open(&dir, now_ms());
            *self.catalogs.write().unwrap() = crate::provider::load_catalogs(&dir);
            *self.routing.write().unwrap() = routing::Prefs::load(&dir);
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
        let active_email = if let Some(p) = active_id.as_deref().and_then(Provider::from_id) {
            Some(p.label().to_string())
        } else {
            active_id
                .as_ref()
                .and_then(|id| s.accounts.iter().find(|a| &a.id == id))
                .and_then(|a| a.email.clone())
        };

        let mut accounts: Vec<AccountView> = s
            .accounts
            .iter()
            .map(|a| AccountView {
                id: a.id.clone(),
                label: a.label.clone(),
                email: a.email.clone(),
                subscription_type: a.subscription_type.clone(),
                provider: "claude".into(),
                models: Vec::new(),
                model: None,
                usage: s.usage.get(&a.id).cloned().unwrap_or_default(),
                is_active: Some(&a.id) == active_id.as_ref(),
                usage_error: s.usage_errors.get(&a.id).cloned(),
                forecasts: self.history.read().unwrap().forecasts(&a.id, now_ms()),
            })
            .collect();
        for p in Provider::ALL {
            if !s.keys.contains_key(&p) {
                continue;
            }
            let catalog = self.catalog(p);
            let model = self.selected_model(p);
            let name = catalog
                .iter()
                .find(|m| m.id == model)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| model.clone());
            let plan = match p {
                Provider::Kimi => "Kimi Code",
                Provider::Refuseless => "Refuseless",
            };
            accounts.push(AccountView {
                id: p.id().into(),
                label: p.label().into(),
                email: None,
                subscription_type: Some(format!("{plan} · {name}")),
                provider: p.id().into(),
                models: catalog,
                model: Some(model),
                usage: s.usage.get(p.id()).cloned().unwrap_or_default(),
                is_active: active_id.as_deref() == Some(p.id()),
                usage_error: s.usage_errors.get(p.id()).cloned(),
                forecasts: self.history.read().unwrap().forecasts(p.id(), now_ms()),
            });
        }

        AppSnapshot {
            accounts,
            active_id,
            active_email,
            auto_switch_enabled: self.prefs.read().unwrap().auto_switch.enabled,
            last_switch: s.last_switch.clone(),
            update: self.update.read().unwrap().clone(),
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
        if let Some(p) = Provider::from_id(id) {
            return self.disconnect_provider(p);
        }
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
        if Provider::from_id(id).is_some() {
            return Ok(());
        }
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

        if let Some(p) = Provider::from_id(id) {
            // The keychain slot stays on the last Claude account: Claude Code
            // still uses it for claude.ai services, sessions pinned to Claude
            // keep running on it, and switching back is then just a matter of
            // clearing the env again.
            self.route_globally(p).with_context(|| {
                format!("pointing Claude Code at {} in settings.json", p.label())
            })?;
            self.state.write().unwrap().active_id = Some(p.id().to_string());
            self.update_tray_readout();
            self.emit();
            return Ok(());
        }

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
        // Leaving a provider: blank its env so running sessions drop back to
        // the keychain login (deleting the keys wouldn't reach them).
        if routing::global_provider().is_some() {
            self.leave_provider_globally()
                .context("switching Claude Code back to Claude in settings.json")?;
        }
        {
            let mut s = self.state.write().unwrap();
            s.active_id = Some(id.to_string());
        }
        self.emit();
        Ok(())
    }

    /// A switch the user asked for. Auto-switch respects it: it won't switch
    /// back to a higher-priority account until this one trips.
    pub async fn set_active_by_user(&self, id: &str) -> Result<()> {
        self.set_active(id).await?;
        let mut s = self.state.write().unwrap();
        s.manual_hold = true;
        s.exhausted_notified = false;
        Ok(())
    }

    // ---- auto-switch ------------------------------------------------------

    /// After a poll: switch accounts if the rules say so.
    async fn auto_switch(&self) {
        let cfg = self.settings().auto_switch;
        if !cfg.enabled {
            return;
        }
        let (decision, active, names) = {
            let s = self.state.read().unwrap();
            let mut candidates: Vec<Candidate> =
                s.accounts.iter().map(|a| candidate(&s, &a.id)).collect();
            for p in Provider::ALL {
                if s.keys.contains_key(&p) {
                    candidates.push(candidate(&s, p.id()));
                }
            }
            let decision = autoswitch::decide(
                &cfg,
                s.active_id.as_deref(),
                &candidates,
                s.manual_hold,
                now_ms(),
            );
            let mut names: HashMap<String, String> = s
                .accounts
                .iter()
                .map(|a| {
                    (
                        a.id.clone(),
                        a.email.clone().unwrap_or_else(|| a.label.clone()),
                    )
                })
                .collect();
            for p in Provider::ALL {
                names.insert(p.id().to_string(), p.label().to_string());
            }
            (decision, s.active_id.clone(), names)
        };
        let Some(from) = active else { return };
        let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());

        match decision {
            Decision::Stay => {
                self.state.write().unwrap().exhausted_notified = false;
            }
            Decision::Exhausted { reason } => {
                let first =
                    !std::mem::replace(&mut self.state.write().unwrap().exhausted_notified, true);
                if first && cfg.notify {
                    self.notify(
                        "No account has room left",
                        &format!(
                            "{}: {reason}. Clyde switches as soon as another account can take over.",
                            name(&from)
                        ),
                    );
                }
            }
            Decision::Switch { to, reason } => {
                let back = reason == Reason::SwitchBack;
                if back {
                    let recent = self
                        .state
                        .read()
                        .unwrap()
                        .last_switch
                        .as_ref()
                        .is_some_and(|e| now_ms() - e.at < SWITCH_BACK_GAP_MS);
                    if recent {
                        return;
                    }
                }
                let why = match &reason {
                    Reason::Tripped(r) => r.clone(),
                    Reason::SwitchBack => "back to the preferred account".to_string(),
                };
                if let Err(e) = self.set_active(&to).await {
                    tracing::warn!("auto-switch to {to} failed: {e:#}");
                    return;
                }
                tracing::info!("auto-switched {from} → {to} ({why})");
                {
                    let mut s = self.state.write().unwrap();
                    s.manual_hold = false;
                    s.exhausted_notified = false;
                    s.last_switch = Some(SwitchEvent {
                        from: from.clone(),
                        to: to.clone(),
                        reason: why.clone(),
                        at: now_ms(),
                    });
                }
                if cfg.notify {
                    let (title, body) = if back {
                        (
                            format!("Back on {}", name(&to)),
                            "Its limits have room again.".to_string(),
                        )
                    } else {
                        (
                            format!("Switched to {}", name(&to)),
                            format!("{}: {why}", name(&from)),
                        )
                    };
                    self.notify(&title, &body);
                }
                self.update_tray_readout();
                self.emit();
            }
        }
    }

    // ---- providers (Kimi, Refuseless) ---------------------------------------

    /// The account to report as active: a provider while settings.json routes
    /// Claude Code there (the keychain then still holds the last Claude login),
    /// else whoever owns the keychain slot.
    fn effective_active(&self, keychain_owner: Option<String>) -> Option<String> {
        match routing::global_provider() {
            Some(p) if self.provider_key(p).is_some() => Some(p.id().to_string()),
            _ => keychain_owner,
        }
    }

    /// The connected provider's key, for the relay.
    pub fn provider_key(&self, p: Provider) -> Option<String> {
        self.state.read().unwrap().keys.get(&p).cloned()
    }

    /// The provider's models: as last read, or the built-in fallback.
    pub fn catalog(&self, p: Provider) -> Vec<ModelInfo> {
        self.catalogs
            .read()
            .unwrap()
            .get(&p)
            .filter(|c| !c.is_empty())
            .cloned()
            .unwrap_or_else(|| p.fallback_models())
    }

    /// The model the provider's aliases point at: the user's pick while the
    /// provider still serves it, else the default, else its first model.
    pub fn selected_model(&self, p: Provider) -> String {
        let catalog = self.catalog(p);
        let has = |id: &str| catalog.iter().any(|m| m.id == id);
        let picked = self.routing.read().unwrap().models.get(&p).cloned();
        match picked {
            Some(m) if has(&m) => m,
            _ if has(p.default_model()) => p.default_model().to_string(),
            _ => catalog
                .first()
                .map(|m| m.id.clone())
                .unwrap_or_else(|| p.default_model().to_string()),
        }
    }

    /// Whether `model` (with or without `[1m]`) is one of a provider's ids.
    fn is_provider_model(&self, model: &str) -> bool {
        let bare = model
            .strip_suffix("[1m]")
            .or_else(|| model.strip_suffix("[1M]"))
            .unwrap_or(model);
        Provider::ALL
            .into_iter()
            .any(|p| self.catalog(p).iter().any(|m| m.id == bare))
    }

    fn save_routing(&self) {
        if let Some(dir) = self.data_dir.read().unwrap().as_ref() {
            if let Err(e) = self.routing.read().unwrap().save(dir) {
                tracing::warn!("couldn't save routing preferences: {e:#}");
            }
        }
    }

    /// Build the route to `p` and hand it to `f`.
    fn with_route<T>(&self, p: Provider, f: impl FnOnce(&ProviderRoute) -> T) -> T {
        let key = self.provider_key(p);
        let catalog = self.catalog(p);
        let selected = self.selected_model(p);
        let model = catalog
            .iter()
            .find(|m| m.id == selected)
            .cloned()
            .unwrap_or_else(|| catalog[0].clone());
        f(&ProviderRoute {
            provider: p,
            key: key.as_deref(),
            catalog: &catalog,
            model: &model,
        })
    }

    fn relay_up(&self) -> bool {
        self.relay_up.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Send every default-config session to `p`.
    fn route_globally(&self, p: Provider) -> Result<()> {
        if self.provider_key(p).is_none() {
            anyhow::bail!("{} isn't connected", p.label());
        }
        let relay = self.relay_up();
        let mut prefs = self.routing.read().unwrap().clone();
        self.with_route(p, |r| {
            routing::enter_provider(&mut prefs, r, relay, |m| self.is_provider_model(m))
        })?;
        *self.routing.write().unwrap() = prefs;
        self.save_routing();
        Ok(())
    }

    /// Send default-config sessions back to Anthropic.
    fn leave_provider_globally(&self) -> Result<()> {
        let mut prefs = self.routing.read().unwrap().clone();
        routing::leave_provider(&mut prefs, |m| self.is_provider_model(m))?;
        *self.routing.write().unwrap() = prefs;
        self.save_routing();
        Ok(())
    }

    /// Rewrite every place that routes to a provider — the global choice,
    /// pinned folders, launch files — for the current keys, models and relay
    /// state. Cheap, and idempotent: files are only written when they change.
    pub fn refresh_routes(&self) {
        let relay = self.relay_up();
        if let Some(p) = routing::global_provider() {
            if self.provider_key(p).is_some() {
                if let Err(e) = self.route_globally(p) {
                    tracing::warn!("couldn't update the {} route: {e:#}", p.label());
                }
            }
        }
        let pins = self.routing.read().unwrap().folder_pins.clone();
        for (folder, target) in pins {
            let res = match target.provider() {
                Some(p) if self.provider_key(p).is_some() => self.with_route(p, |r| {
                    routing::pin_folder(std::path::Path::new(&folder), Some(r), relay)
                }),
                Some(_) => continue,
                None => routing::pin_folder(std::path::Path::new(&folder), None, relay),
            };
            if let Err(e) = res {
                tracing::warn!("couldn't update the pin for {folder}: {e:#}");
            }
        }
        for t in Target::ALL {
            let settings = match t.provider() {
                Some(p) if self.provider_key(p).is_some() => {
                    self.with_route(p, |r| routing::launch_settings(t, Some(r), relay))
                }
                Some(_) => None,
                None => routing::launch_settings(t, None, relay),
            };
            if let Err(e) = routing::write_launch_file(t, settings) {
                tracing::warn!("couldn't write the {} launch file: {e:#}", t.id());
            }
        }
    }

    /// Point `p`'s aliases at `model` — in the global route, pinned folders and
    /// launch files — so sessions on `p` follow within seconds.
    pub fn set_provider_model(&self, p: Provider, model: &str) -> Result<()> {
        if !self.catalog(p).iter().any(|m| m.id == model) {
            anyhow::bail!("{} doesn't offer {model}", p.label());
        }
        self.routing
            .write()
            .unwrap()
            .models
            .insert(p, model.to_string());
        self.save_routing();
        self.refresh_routes();
        self.emit();
        Ok(())
    }

    /// Connect a provider with a key from its console, after checking the
    /// provider accepts it.
    pub async fn connect_provider(&self, p: Provider, raw_key: &str) -> Result<()> {
        let key = p.normalize_key(raw_key)?;
        let usage = p.verify_key(&self.http, &key).await?;
        p.save_key(&key)?;
        {
            let mut s = self.state.write().unwrap();
            s.keys.insert(p, key.clone());
            s.usage_errors.remove(p.id());
        }
        self.refresh_catalog(p, &key).await;
        // Paid per token: auto-switch only moves there if the user says so.
        if p.pay_per_token() {
            let mut prefs = self.settings();
            if !prefs.auto_switch.accounts.contains_key(p.id()) {
                prefs
                    .auto_switch
                    .accounts
                    .entry(p.id().to_string())
                    .or_default()
                    .exclude = true;
                let _ = self.set_settings(prefs);
            }
        }
        // A replaced key must reach the settings files too, or sessions keep
        // sending the old one (when not going through the relay).
        self.refresh_routes();
        self.record_usage(p.id(), usage);
        Ok(())
    }

    /// Forget a provider's key; if Claude Code was on it, put it back on the
    /// keychain login. Folders pinned to it lose their pin.
    fn disconnect_provider(&self, p: Provider) -> Result<()> {
        if routing::global_provider() == Some(p) {
            self.leave_provider_globally()?;
        }
        let pinned: Vec<String> = self
            .routing
            .read()
            .unwrap()
            .folder_pins
            .iter()
            .filter(|(_, t)| t.provider() == Some(p))
            .map(|(f, _)| f.clone())
            .collect();
        for folder in pinned {
            let _ = self.set_folder_pin(&folder, None);
        }
        p.delete_key()?;
        {
            let mut s = self.state.write().unwrap();
            s.keys.remove(&p);
            s.usage.remove(p.id());
            s.usage_errors.remove(p.id());
        }
        self.refresh_routes();
        let owner = self.adopt_active_slot();
        self.state.write().unwrap().active_id = owner;
        self.emit();
        Ok(())
    }

    /// Pin a folder's sessions to `target`, or follow the global choice again
    /// with `None`. Applies live to sessions running there.
    pub fn set_folder_pin(&self, folder: &str, target: Option<Target>) -> Result<()> {
        let path = std::path::Path::new(folder);
        if !path.is_absolute() || !path.is_dir() {
            anyhow::bail!("{folder} isn't a folder");
        }
        let relay = self.relay_up();
        match target {
            Some(t) => {
                match t.provider() {
                    Some(p) => {
                        if self.provider_key(p).is_none() {
                            anyhow::bail!("{} isn't connected", p.label());
                        }
                        self.with_route(p, |r| routing::pin_folder(path, Some(r), relay))?;
                    }
                    None => routing::pin_folder(path, None, relay)?,
                }
                self.routing
                    .write()
                    .unwrap()
                    .folder_pins
                    .insert(folder.to_string(), t);
            }
            None => {
                routing::unpin_folder(path)?;
                self.routing.write().unwrap().folder_pins.remove(folder);
                // A key that's simply gone doesn't reach a running session:
                // make sure the user's env names every one, blank on Claude.
                if routing::global_provider().is_none() {
                    claude_sync::edit_settings_env(|env| {
                        let mut changed = false;
                        for k in crate::provider::ENV_KEYS {
                            if !env.contains_key(*k) {
                                env.insert(
                                    (*k).to_string(),
                                    serde_json::Value::String(String::new()),
                                );
                                changed = true;
                            }
                        }
                        changed
                    })?;
                }
            }
        }
        self.save_routing();
        Ok(())
    }

    pub fn folder_pins(&self) -> std::collections::BTreeMap<String, Target> {
        self.routing.read().unwrap().folder_pins.clone()
    }

    /// `/model` picked a provider's model and saved it as the default: adopt
    /// it as that provider's chosen model when it's the global one, and put
    /// the user's own `model` setting back either way. Runs every few seconds,
    /// but only reads `settings.json` when it changed.
    pub fn watch_model_setting(&self) {
        let Ok(path) = claude_sync::settings_path() else {
            return;
        };
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        {
            let mut seen = self.settings_seen.lock().unwrap();
            if *seen == modified {
                return;
            }
            *seen = modified;
        }
        let mut prefs = self.routing.read().unwrap().clone();
        let (picked, changed) =
            match routing::reconcile_model_setting(&mut prefs, |m| self.is_provider_model(m)) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("couldn't check the model setting: {e:#}");
                    return;
                }
            };
        if changed || picked.is_some() {
            *self.routing.write().unwrap() = prefs;
            self.save_routing();
        }
        let Some(picked) = picked else { return };
        let bare = picked.trim_end_matches("[1m]").to_string();
        match routing::global_provider() {
            Some(p) if self.catalog(p).iter().any(|m| m.id == bare) => {
                tracing::info!("/model picked {bare}: {}'s chosen model now", p.label());
                if let Err(e) = self.set_provider_model(p, &bare) {
                    tracing::warn!("couldn't adopt {bare}: {e:#}");
                }
            }
            _ => tracing::info!("put the model setting back after /model saved {picked}"),
        }
    }

    /// Keep trying to read the saved accounts if startup couldn't; once they
    /// load, take over as if they'd been there all along.
    pub fn retry_vault_load(self: &Arc<Self>) {
        if vault::is_loaded() {
            return;
        }
        let core = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            match vault::load_accounts() {
                Ok(accounts) => {
                    tracing::info!("read {} saved accounts from the Keychain", accounts.len());
                    core.state.write().unwrap().accounts = accounts;
                    core.detect_active();
                    let c = core.clone();
                    tauri::async_runtime::spawn(async move { c.poll_usage().await });
                    return;
                }
                Err(e) => tracing::debug!("saved accounts still unreadable: {e:#}"),
            }
        });
    }

    /// Read the providers' keys from the Keychain in the background, then
    /// route through the relay wherever Claude Code is on one. A Keychain read
    /// can wait on a permission prompt (after an update changes Clyde's
    /// signature), and that must not stall startup.
    pub fn load_provider_keys(self: &Arc<Self>) {
        let core = self.clone();
        std::thread::spawn(move || {
            let mut any = false;
            for p in Provider::ALL {
                let Some(key) = p.key() else { continue };
                // Re-create the item as Clyde's own (the way the account list is
                // stored), so it stays readable across updates even if
                // something else created it.
                p.adopt_key(&key);
                core.state.write().unwrap().keys.insert(p, key);
                any = true;
            }
            if !any {
                return;
            }
            core.detect_active();
            core.refresh_routes();
            core.emit();
            let c = core.clone();
            tauri::async_runtime::spawn(async move { c.poll_providers().await });
        });
    }

    /// Note whether the relay is up; provider env written from now on uses it.
    pub fn set_relay_running(&self, up: bool) {
        self.relay_up
            .store(up, std::sync::atomic::Ordering::Relaxed);
    }

    /// Clyde is quitting: its relay goes with it, so every route through the
    /// relay is switched to go straight to the provider.
    pub fn on_exit(&self) {
        if !self.relay_up() {
            return;
        }
        self.set_relay_running(false);
        self.refresh_routes();
        tracing::info!("Clyde is quitting: pointed provider sessions straight at the providers");
    }

    /// Re-read a provider's model list (hourly, or on demand).
    async fn refresh_catalog(&self, p: Provider, key: &str) {
        match p.fetch_models(&self.http, key).await {
            Ok(models) => {
                let changed = self.catalogs.read().unwrap().get(&p) != Some(&models);
                self.catalog_read_at.write().unwrap().insert(p, now_ms());
                if changed {
                    self.catalogs.write().unwrap().insert(p, models);
                    if let Some(dir) = self.data_dir.read().unwrap().as_ref() {
                        let _ = crate::provider::save_catalogs(dir, &self.catalogs.read().unwrap());
                    }
                    self.refresh_routes();
                }
            }
            Err(e) => tracing::debug!("{} model list failed: {e:#}", p.label()),
        }
    }

    async fn poll_providers(&self) {
        for p in Provider::ALL {
            let Some(key) = self.provider_key(p) else {
                continue;
            };
            let stale = self
                .catalog_read_at
                .read()
                .unwrap()
                .get(&p)
                .is_none_or(|t| now_ms() - t > 60 * 60 * 1000);
            if stale {
                self.refresh_catalog(p, &key).await;
            }
            match p.fetch_usage(&self.http, &key).await {
                Ok(snap) => self.record_usage(p.id(), snap),
                Err(e) => {
                    tracing::debug!("{} usage fetch failed: {e:#}", p.label());
                    let msg = format!("{e:#}");
                    let short = if msg.contains("401") || msg.contains("403") {
                        format!(
                            "Signed out — {} rejected the key; connect it again",
                            p.label()
                        )
                    } else {
                        format!("Couldn't read {} usage", p.label())
                    };
                    self.record_usage_error(p.id(), short);
                }
            }
        }
    }

    /// Find an account by id, email, label (case-insensitive), or a provider
    /// (`kimi`, optionally with a model: `kimi:k3`) — for `clyde --switch <who>`.
    pub fn resolve_account(&self, query: &str) -> Option<String> {
        let q = query.trim().to_lowercase();
        let (who, model) = q
            .split_once(':')
            .map_or((q.as_str(), None), |(w, m)| (w, Some(m)));
        if let Some(p) = Provider::from_id(who) {
            self.provider_key(p)?;
            if let Some(m) = model {
                if let Err(e) = self.set_provider_model(p, m) {
                    tracing::warn!("--switch {query}: {e:#}");
                }
            }
            return Some(p.id().to_string());
        }
        let s = self.state.read().unwrap();
        s.accounts
            .iter()
            .find(|a| {
                a.id.to_lowercase() == q
                    || a.email.as_deref().is_some_and(|e| e.to_lowercase() == q)
                    || a.label.to_lowercase() == q
            })
            .map(|a| a.id.clone())
    }

    // ---- updates ----------------------------------------------------------

    /// Record a newer release (or none) and tell the UI. Returns whether this
    /// version is news, so the caller notifies only once per version.
    pub fn set_update(&self, info: Option<UpdateInfo>) -> bool {
        let new = {
            let mut u = self.update.write().unwrap();
            let new = match (&*u, &info) {
                (Some(old), Some(i)) => old.version != i.version,
                (None, Some(_)) => true,
                _ => false,
            };
            *u = info;
            new
        };
        self.emit();
        new
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
        let resolved = self.effective_active(self.adopt_active_slot());
        {
            let mut s = self.state.write().unwrap();
            s.active_id = resolved;
        }
        // Blanked provider keys only exist for sessions that saw a provider;
        // with none running they're clutter.
        if routing::global_provider().is_none() && claude_sync::running_claude_sessions() == 0 {
            if let Err(e) = claude_sync::edit_settings_env(crate::provider::tidy) {
                tracing::debug!("couldn't tidy settings.json: {e:#}");
            }
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
            let name = if let Some(p) = Provider::from_id(account_id) {
                p.label().to_string()
            } else {
                s.accounts
                    .iter()
                    .find(|a| a.id == account_id)
                    .map(|a| a.email.clone().unwrap_or_else(|| a.label.clone()))
                    .unwrap_or_default()
            };
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
        if let Some(id) = self.effective_active(self.adopt_active_slot()) {
            let changed = {
                let mut s = self.state.write().unwrap();
                if s.active_id.as_deref() != Some(id.as_str()) {
                    s.active_id = Some(id);
                    // Switched outside Clyde (`claude /login`): a choice to respect.
                    s.manual_hold = true;
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
        self.poll_providers().await;

        // The backfill may have just learned the active account's uuid, which
        // startup didn't have to write into Claude Code's config.
        self.repair_active_identity();

        self.auto_switch().await;
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

/// What auto-switch knows about one account: its latest clean reading and
/// whether its login still works.
fn candidate<'a>(s: &'a CoreState, id: &'a str) -> Candidate<'a> {
    Candidate {
        id,
        usage: s.usage.get(id).filter(|_| !s.usage_errors.contains_key(id)),
        healthy: !s
            .usage_errors
            .get(id)
            .is_some_and(|e| e.contains("refresh") || e.contains("Signed out")),
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

/// What the relay reads from Clyde for each request.
impl crate::relay::Directory for Core {
    fn key(&self, p: Provider) -> Option<String> {
        self.provider_key(p)
    }

    fn selected(&self, p: Provider) -> String {
        self.selected_model(p)
    }

    fn catalog(&self, p: Provider) -> Vec<ModelInfo> {
        Core::catalog(self, p)
    }
}
