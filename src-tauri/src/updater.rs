//! New-version checks and one-click installs, through Tauri's updater.
//!
//! Releases are built by `.github/workflows/release.yml`: tauri-action signs the
//! update bundle with the key in the `TAURI_SIGNING_PRIVATE_KEY` secret and
//! publishes `latest.json` beside it. Clyde fetches that manifest from the
//! latest GitHub release, checks the signature against the public key in
//! `tauri.conf.json`, and only then swaps itself. Nothing installs without the
//! user clicking Install.

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

use crate::engine::SharedCore;

/// How often to look for a new version while Clyde runs.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Wait this long after launch before the first check, so it doesn't compete
/// with startup work.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(20);

/// A release newer than the running one.
#[derive(Clone, Debug, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current: String,
    /// The release notes, when the release has any.
    pub notes: Option<String>,
}

/// Ask GitHub whether there's a newer release.
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>> {
    let update = app.updater()?.check().await?;
    Ok(update.map(|u| UpdateInfo {
        version: u.version.clone(),
        current: u.current_version.clone(),
        notes: u.body.clone().filter(|b| !b.trim().is_empty()),
    }))
}

/// Download, verify and install the newer release, then relaunch into it.
pub async fn install(app: &AppHandle) -> Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        anyhow::bail!("Clyde is already up to date");
    };
    tracing::info!("installing Clyde {}", update.version);
    update.download_and_install(|_, _| {}, || {}).await?;
    app.restart();
}

/// Check now and then for as long as Clyde runs, telling the user once per
/// new version. Respects Settings → "Check for updates".
pub fn spawn_background_checks(app: AppHandle, core: SharedCore) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_AFTER).await;
        loop {
            if core.settings().check_updates {
                match check(&app).await {
                    Ok(info) => {
                        let version = info.as_ref().map(|i| i.version.clone());
                        if core.set_update(info) {
                            if let Some(v) = version {
                                core.notify(
                                    &format!("Clyde {v} is available"),
                                    "Open Clyde and click Install to update.",
                                );
                            }
                        }
                    }
                    Err(e) => tracing::debug!("update check failed: {e:#}"),
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}
