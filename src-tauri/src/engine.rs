//! The Clyde engine: shared, in-memory source of truth used by both the proxy
//! and the Tauri command layer. Holds accounts, routing mode, live usage, and
//! the token-refresh machinery.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Result};
use tauri::{AppHandle, Emitter};

use crate::model::*;
use crate::{oauth, vault};

/// Switch away from the active account once its utilization crosses this
/// percentage (pre-emptive routing, before a hard 429 ever lands).
const SWITCH_THRESHOLD: f64 = 90.0;

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
}

struct CoreState {
    accounts: Vec<Account>,
    mode: Mode,
    usage: HashMap<String, UsageSnapshot>,
    active_id: Option<String>,
    proxy_port: u16,
    proxy_running: bool,
    integration_enabled: bool,
}

impl Core {
    pub fn new(proxy_port: u16) -> Result<SharedCore> {
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
            state: RwLock::new(CoreState {
                accounts,
                mode: Mode::Auto,
                usage: HashMap::new(),
                active_id,
                proxy_port,
                proxy_running: false,
                integration_enabled: false,
            }),
        }))
    }

    pub fn attach_app(&self, app: AppHandle) {
        *self.app.write().unwrap() = Some(app);
    }

    // ---- snapshot / UI ----------------------------------------------------

    pub fn snapshot(&self) -> AppSnapshot {
        let s = self.state.read().unwrap();
        let active_id = self.resolve_active(&s);
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
            })
            .collect();

        AppSnapshot {
            accounts,
            mode: s.mode.clone(),
            active_id,
            proxy_port: s.proxy_port,
            proxy_running: s.proxy_running,
            integration_enabled: s.integration_enabled,
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
            if s.active_id.as_deref() == Some(id) {
                s.active_id = s.accounts.first().map(|a| a.id.clone());
            }
            if s.mode == Mode::Pinned(id.to_string()) {
                s.mode = Mode::Auto;
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

    pub fn set_mode(&self, mode: Mode) {
        {
            let mut s = self.state.write().unwrap();
            s.mode = mode;
        }
        self.emit();
    }

    pub fn set_proxy_running(&self, running: bool) {
        {
            let mut s = self.state.write().unwrap();
            s.proxy_running = running;
        }
        self.emit();
    }

    pub fn set_integration_enabled(&self, enabled: bool) {
        {
            let mut s = self.state.write().unwrap();
            s.integration_enabled = enabled;
        }
        self.emit();
    }

    pub fn proxy_port(&self) -> u16 {
        self.state.read().unwrap().proxy_port
    }

    #[allow(dead_code)] // used by the tray/menubar status (wired up next)
    pub fn has_accounts(&self) -> bool {
        !self.state.read().unwrap().accounts.is_empty()
    }

    // ---- routing ----------------------------------------------------------

    /// Pick the account that should serve the next request, honoring the mode,
    /// current utilization, and a stickiness threshold to avoid flapping.
    pub fn choose_account(&self) -> Option<String> {
        let s = self.state.read().unwrap();
        self.resolve_active(&s)
    }

    /// Pick an account to fail over to, excluding the ones already tried this
    /// request. Returns the least-pressured remaining (non-limited) account.
    pub fn choose_failover_excluding(&self, exclude: &[String]) -> Option<String> {
        let s = self.state.read().unwrap();
        if let Mode::Pinned(_) = s.mode {
            return None; // pinned mode never fails over
        }
        s.accounts
            .iter()
            .filter(|a| !exclude.contains(&a.id) && !usage_limited(&s.usage, &a.id))
            .min_by(|a, b| {
                pressure(&s.usage, &a.id)
                    .partial_cmp(&pressure(&s.usage, &b.id))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|a| a.id.clone())
    }

    fn resolve_active(&self, s: &CoreState) -> Option<String> {
        match &s.mode {
            Mode::Pinned(id) => {
                if s.accounts.iter().any(|a| &a.id == id) {
                    Some(id.clone())
                } else {
                    None
                }
            }
            Mode::Auto => {
                if s.accounts.is_empty() {
                    return None;
                }
                // Prefer accounts that aren't hard-limited right now.
                let not_limited: Vec<&Account> = s
                    .accounts
                    .iter()
                    .filter(|a| !usage_limited(&s.usage, &a.id))
                    .collect();
                let pool: Vec<&Account> = if not_limited.is_empty() {
                    s.accounts.iter().collect()
                } else {
                    not_limited
                };

                // Stickiness: keep the current active account while it's healthy.
                if let Some(active) = &s.active_id {
                    if pool.iter().any(|a| &a.id == active)
                        && !usage_limited(&s.usage, active)
                        && pressure(&s.usage, active) < SWITCH_THRESHOLD
                    {
                        return Some(active.clone());
                    }
                }

                // Otherwise pick the least-pressured (or soonest-reset if all maxed).
                pool.iter()
                    .min_by(|a, b| {
                        pressure(&s.usage, &a.id)
                            .partial_cmp(&pressure(&s.usage, &b.id))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|a| a.id.clone())
            }
        }
    }

    // ---- usage + tokens ---------------------------------------------------

    pub fn record_usage(&self, account_id: &str, snap: UsageSnapshot) {
        let switched;
        {
            let mut s = self.state.write().unwrap();
            s.usage.insert(account_id.to_string(), snap);
            let new_active = self.resolve_active(&s);
            switched = new_active != s.active_id;
            if let Some(active) = new_active {
                s.active_id = Some(active);
            }
        }
        if switched {
            self.notify_switch();
        }
        self.emit();
    }

    pub fn mark_limited(&self, account_id: &str) {
        {
            let mut s = self.state.write().unwrap();
            let entry = s.usage.entry(account_id.to_string()).or_default();
            entry.status = Some("rejected".to_string());
            entry.five_hour_utilization = Some(100.0);
            entry.updated_at = now_ms();
            let new_active = self.resolve_active(&s);
            s.active_id = new_active;
        }
        self.notify_switch();
        self.emit();
    }

    /// Return a valid (fresh) access token for `account_id`, refreshing if it's
    /// stale and persisting the rotated credential.
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
        if let Some(cred) = self.credential(account_id) {
            if !cred.is_stale(now_ms(), REFRESH_SKEW_MS) {
                return Ok(cred.access_token); // someone else refreshed
            }
            let refreshed = oauth::refresh(&self.http, &cred.refresh_token).await?;
            let token = refreshed.access_token.clone();
            self.update_credential(account_id, refreshed)?;
            Ok(token)
        } else {
            Err(anyhow!("unknown account {account_id}"))
        }
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

    fn notify_switch(&self) {
        // The UI shows the switch; a desktop notification is emitted from the
        // command/proxy layer where the AppHandle's notification API lives.
        self.emit();
    }
}

fn pressure(usage: &HashMap<String, UsageSnapshot>, id: &str) -> f64 {
    usage.get(id).map(|u| u.pressure()).unwrap_or(0.0)
}

fn usage_limited(usage: &HashMap<String, UsageSnapshot>, id: &str) -> bool {
    usage.get(id).map(|u| u.is_limited()).unwrap_or(false)
}
