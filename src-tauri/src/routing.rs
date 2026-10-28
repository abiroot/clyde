//! Where Claude Code's requests go: the global choice, per-folder and launch
//! pins, and the guards that keep a provider's model names away from Anthropic.
//!
//! - **Global.** `~/.claude/settings.json` → `env` routes every session on the
//!   default config to a provider (see [`crate::provider`]), and `modelPicker`
//!   puts the provider's models in `/model` under their own names.
//! - **Folder pins.** The same `env` in a folder's `.claude/settings.local.json`
//!   outranks the user's and applies live to every session running in that
//!   folder (verified on Claude Code 2.1.312). A folder pinned to Claude gets
//!   the keys blanked, which keeps it on Anthropic while the global choice is a
//!   provider.
//! - **Launch pins.** `claude --settings ~/.claude/clyde/<target>.json` (or the
//!   `claude-<target>` commands) starts one session on a target whatever the
//!   global choice — flag settings outrank the user's too, and combine with a
//!   wrapper's own `--settings` (cmux's). Read at launch only.
//!
//! Auto-switch only ever moves the global choice, so pinned sessions stay put.
//!
//! `/model` saves its pick as the user's `model` setting. A provider's model
//! id there would start every new Claude session on a name Anthropic doesn't
//! know, so Clyde takes such a pick as the provider's chosen model and puts the
//! user's own value back. A *running* session keeps a model it picked by id
//! across a switch back to Claude; an `availableModels` list of Claude's
//! families moves it to the default instead of an error (verified live).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::claude_sync;
use crate::provider::{self, ModelInfo, Provider};

/// Claude's model families: the `availableModels` guard while on Claude.
const CLAUDE_FAMILIES: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

/// Where a pinned folder or launch sends Claude Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// Anthropic, as whichever Claude account is active.
    Claude,
    Kimi,
    Refuseless,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Claude, Target::Kimi, Target::Refuseless];

    pub fn provider(self) -> Option<Provider> {
        match self {
            Target::Claude => None,
            Target::Kimi => Some(Provider::Kimi),
            Target::Refuseless => Some(Provider::Refuseless),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Target::Claude => "claude",
            Target::Kimi => "kimi",
            Target::Refuseless => "refuseless",
        }
    }

    pub fn parse(s: &str) -> Option<Target> {
        Target::ALL
            .into_iter()
            .find(|t| t.id().eq_ignore_ascii_case(s.trim()))
    }

    /// The terminal command that starts a session pinned here.
    pub fn command_name(self) -> &'static str {
        match self {
            Target::Claude => "claude-anthropic",
            Target::Kimi => "claude-kimi",
            Target::Refuseless => "claude-refuseless",
        }
    }
}

/// Clyde's own bookkeeping for routing, in the app data dir.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The model each provider's aliases point at.
    pub models: BTreeMap<Provider, String>,
    /// The user's own `model` setting (`None`: unset), put back whenever a
    /// provider's model id lands there.
    pub claude_model: Option<String>,
    /// The `availableModels` value Clyde wrote, so it only ever removes its own.
    pub guard: Option<Value>,
    /// Clyde's lineup is in `modelPicker`; `saved_picker` is what was there before.
    pub picker_set: bool,
    pub saved_picker: Option<Value>,
    /// Folder (absolute path) → where its sessions go.
    pub folder_pins: BTreeMap<String, Target>,
}

impl Prefs {
    pub fn load(dir: &Path) -> Prefs {
        std::fs::read_to_string(dir.join("routing.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("routing.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// Everything needed to point Claude Code at one provider.
pub struct ProviderRoute<'a> {
    pub provider: Provider,
    pub key: Option<&'a str>,
    pub catalog: &'a [ModelInfo],
    pub model: &'a ModelInfo,
}

impl ProviderRoute<'_> {
    /// The env for this provider: through the relay when it's up, else
    /// straight to the provider with the real key (`None` without a key).
    pub fn env(&self, relay_up: bool) -> Option<Vec<(&'static str, String)>> {
        let (base_url, token) = if relay_up {
            (
                crate::relay::base_url(self.provider, None),
                crate::relay::PLACEHOLDER_TOKEN.to_string(),
            )
        } else {
            (
                format!("{}/", self.provider.upstream()),
                self.key?.to_string(),
            )
        };
        Some(provider::env_on(&provider::Route {
            provider: self.provider,
            base_url,
            token,
            model: self.model,
            catalog: self.catalog,
        }))
    }

    fn allowed_models(&self) -> Value {
        let mut ids: Vec<String> = CLAUDE_FAMILIES.iter().map(|s| s.to_string()).collect();
        ids.extend(self.catalog.iter().map(|m| m.id.clone()));
        json!(ids)
    }
}

fn model_setting(root: &Map<String, Value>) -> Option<String> {
    root.get("model").and_then(Value::as_str).map(String::from)
}

fn set_model_setting(root: &mut Map<String, Value>, value: Option<&String>) {
    match value {
        Some(v) => {
            root.insert("model".into(), json!(v));
        }
        None => {
            root.remove("model");
        }
    }
}

// ---- global ----------------------------------------------------------------

/// Route every default-config session to `route` (live).
pub fn enter_provider(
    prefs: &mut Prefs,
    route: &ProviderRoute,
    relay_up: bool,
    is_provider_model: impl Fn(&str) -> bool,
) -> Result<()> {
    enter_provider_at(
        &claude_sync::settings_path()?,
        prefs,
        route,
        relay_up,
        is_provider_model,
    )
}

fn enter_provider_at(
    path: &Path,
    prefs: &mut Prefs,
    route: &ProviderRoute,
    relay_up: bool,
    is_provider_model: impl Fn(&str) -> bool,
) -> Result<()> {
    let Some(env) = route.env(relay_up) else {
        anyhow::bail!("{} isn't connected", route.provider.label());
    };
    claude_sync::edit_settings_file(path, |root| {
        let before = root.clone();
        claude_sync::edit_env(root, |e| claude_sync::set_pairs(e, &env))?;
        if !prefs.picker_set {
            prefs.saved_picker = root.get("modelPicker").cloned();
            prefs.picker_set = true;
        }
        root.insert(
            "modelPicker".into(),
            provider::model_picker(route.provider, route.catalog),
        );
        if prefs.guard.is_some() && root.get("availableModels") == prefs.guard.as_ref() {
            root.remove("availableModels");
        }
        prefs.guard = None;
        match model_setting(root) {
            Some(m) if is_provider_model(&m) => {}
            other => prefs.claude_model = other,
        }
        Ok(*root != before)
    })?;
    Ok(())
}

/// Put default-config sessions back on Anthropic (live): blank the provider
/// env, restore the picker and the user's `model`, and guard against sessions
/// that picked a provider's model by id.
pub fn leave_provider(prefs: &mut Prefs, is_provider_model: impl Fn(&str) -> bool) -> Result<()> {
    leave_provider_at(&claude_sync::settings_path()?, prefs, is_provider_model)
}

fn leave_provider_at(
    path: &Path,
    prefs: &mut Prefs,
    is_provider_model: impl Fn(&str) -> bool,
) -> Result<()> {
    claude_sync::edit_settings_file(path, |root| {
        let before = root.clone();
        claude_sync::edit_env(root, |e| claude_sync::set_pairs(e, &provider::env_off()))?;
        if prefs.picker_set {
            match prefs.saved_picker.take() {
                Some(p) => root.insert("modelPicker".into(), p),
                None => root.remove("modelPicker"),
            };
            prefs.picker_set = false;
        }
        if model_setting(root).is_some_and(|m| is_provider_model(&m)) {
            set_model_setting(root, prefs.claude_model.as_ref());
        }
        if !root.contains_key("availableModels") {
            let guard = json!(CLAUDE_FAMILIES);
            root.insert("availableModels".into(), guard.clone());
            prefs.guard = Some(guard);
        }
        Ok(*root != before)
    })?;
    Ok(())
}

/// After `/model` saved a pick: a provider's model id in the user's `model`
/// setting goes back to the user's own value, and is returned so the caller
/// can adopt it when that provider is the active one. A Claude value is
/// remembered as the one to restore.
pub fn reconcile_model_setting(
    prefs: &mut Prefs,
    is_provider_model: impl Fn(&str) -> bool,
) -> Result<(Option<String>, bool)> {
    reconcile_model_setting_at(&claude_sync::settings_path()?, prefs, is_provider_model)
}

fn reconcile_model_setting_at(
    path: &Path,
    prefs: &mut Prefs,
    is_provider_model: impl Fn(&str) -> bool,
) -> Result<(Option<String>, bool)> {
    let mut picked = None;
    let mut prefs_changed = false;
    claude_sync::edit_settings_file(path, |root| {
        let current = model_setting(root);
        match current {
            Some(m) if is_provider_model(&m) => {
                set_model_setting(root, prefs.claude_model.as_ref());
                picked = Some(m);
                Ok(true)
            }
            other => {
                if other != prefs.claude_model {
                    prefs.claude_model = other;
                    prefs_changed = true;
                }
                Ok(false)
            }
        }
    })?;
    Ok((picked, prefs_changed))
}

/// The provider settings.json currently routes default-config sessions to.
pub fn global_provider() -> Option<Provider> {
    provider::routed_provider(&claude_sync::read_settings_env())
}

// ---- folder pins -----------------------------------------------------------

/// Keys Clyde owns in a pinned folder's local settings.
fn strip_pin(root: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    if let Some(Value::Object(env)) = root.get_mut("env") {
        let before = env.len();
        env.retain(|k, _| !provider::ENV_KEYS.contains(&k.as_str()));
        changed |= env.len() != before;
        if env.is_empty() {
            root.remove("env");
        }
    }
    if root.get("availableModels").is_some_and(is_our_allowlist) {
        root.remove("availableModels");
        changed = true;
    }
    changed
}

fn is_our_allowlist(v: &Value) -> bool {
    v.as_array().is_some_and(|a| {
        a.iter()
            .take(CLAUDE_FAMILIES.len())
            .map(Value::as_str)
            .eq(CLAUDE_FAMILIES.iter().map(|s| Some(*s)))
    })
}

/// Pin `folder` to `route` (a provider), or to Claude with `None` (live).
pub fn pin_folder(folder: &Path, route: Option<&ProviderRoute>, relay_up: bool) -> Result<()> {
    let path = claude_sync::local_settings_path(folder);
    claude_sync::edit_settings_file(&path, |root| {
        let before = root.clone();
        match route {
            Some(r) => {
                let Some(env) = r.env(relay_up) else {
                    anyhow::bail!("{} isn't connected", r.provider.label());
                };
                claude_sync::edit_env(root, |e| claude_sync::set_pairs(e, &env))?;
                root.insert("availableModels".into(), r.allowed_models());
            }
            None => {
                claude_sync::edit_env(root, |e| claude_sync::set_pairs(e, &provider::env_off()))?;
                if root.get("availableModels").is_some_and(is_our_allowlist) {
                    root.remove("availableModels");
                }
            }
        }
        Ok(*root != before)
    })?;
    Ok(())
}

/// Remove a folder's pin. Running sessions there then follow the global
/// choice again — the caller makes sure the user's env names every key, since
/// a key that's simply gone doesn't reach a running session.
pub fn unpin_folder(folder: &Path) -> Result<()> {
    let path = claude_sync::local_settings_path(folder);
    if !path.exists() {
        return Ok(());
    }
    claude_sync::edit_settings_file(&path, |root| Ok(strip_pin(root)))?;
    // Clyde may have created the file just for the pin.
    if claude_sync::read_settings_file(&path).is_empty() {
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

// ---- launch pins -------------------------------------------------------------

pub fn launch_dir() -> Result<PathBuf> {
    Ok(claude_sync::claude_dir()?.join("clyde"))
}

pub fn launch_file(target: Target) -> Result<PathBuf> {
    Ok(launch_dir()?.join(format!("{}.json", target.id())))
}

/// The `--settings` file for a launch pin.
pub fn launch_settings(
    target: Target,
    route: Option<&ProviderRoute>,
    relay_up: bool,
) -> Option<Value> {
    match (target.provider(), route) {
        (None, _) => {
            let env: Map<String, Value> = provider::env_off()
                .into_iter()
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect();
            // Claude's own lineup, whatever the user settings' picker holds
            // while a provider is the global choice.
            Some(json!({
                "env": env,
                "modelPicker": {"options": CLAUDE_FAMILIES.iter().map(|f| json!({"model": f})).collect::<Vec<_>>()},
            }))
        }
        (Some(_), Some(r)) => {
            let env: Map<String, Value> = r
                .env(relay_up)?
                .into_iter()
                .map(|(k, v)| (k.to_string(), json!(v)))
                .collect();
            Some(json!({
                "env": env,
                "modelPicker": provider::model_picker(r.provider, r.catalog),
                "availableModels": r.allowed_models(),
            }))
        }
        (Some(_), None) => None,
    }
}

/// Write (or, for a disconnected provider, remove) a launch pin's file.
pub fn write_launch_file(target: Target, settings: Option<Value>) -> Result<()> {
    let path = launch_file(target)?;
    match settings {
        Some(s) => {
            let current: Option<Value> = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok());
            if current.as_ref() == Some(&s) {
                return Ok(());
            }
            std::fs::create_dir_all(launch_dir()?)?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_string_pretty(&s)?)?;
            set_private(&tmp);
            std::fs::rename(&tmp, &path)?;
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private(_path: &Path) {}

/// `~/.local/bin`, where the `claude-<target>` commands go.
fn bin_dir() -> Result<PathBuf> {
    Ok(claude_sync::claude_dir()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
        .join(".local/bin"))
}

const COMMAND_MARKER: &str = "# Added by Clyde";

fn command_script(target: Target) -> String {
    format!(
        "#!/bin/sh\n{COMMAND_MARKER}: start Claude Code on {}, whatever Clyde's global choice.\n\
         exec claude --settings \"$HOME/.claude/clyde/{}.json\" \"$@\"\n",
        match target {
            Target::Claude => "Claude (the active Claude account)".to_string(),
            t => t
                .provider()
                .map(|p| p.label().to_string())
                .unwrap_or_default(),
        },
        target.id()
    )
}

/// Install the `claude-<target>` commands. Never overwrites a file Clyde
/// didn't write.
pub fn install_commands() -> Result<Vec<String>> {
    let dir = bin_dir()?;
    std::fs::create_dir_all(&dir)?;
    let mut installed = Vec::new();
    for t in Target::ALL {
        let path = dir.join(t.command_name());
        if let Ok(existing) = std::fs::read_to_string(&path) {
            if !existing.contains(COMMAND_MARKER) {
                continue;
            }
        }
        std::fs::write(&path, command_script(t))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
        installed.push(t.command_name().to_string());
    }
    Ok(installed)
}

pub fn commands_installed() -> bool {
    bin_dir()
        .map(|d| {
            Target::ALL.iter().all(|t| {
                std::fs::read_to_string(d.join(t.command_name()))
                    .is_ok_and(|s| s.contains(COMMAND_MARKER))
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kimi_catalog() -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: "k3.1-pro".into(),
            name: "K3.1 Pro".into(),
            context: 262_144,
            efforts: vec!["low".into(), "high".into(), "max".into()],
            thinking_only: true,
        }]
    }

    #[test]
    fn launch_settings_pin_a_provider_or_claude() {
        let catalog = kimi_catalog();
        let route = ProviderRoute {
            provider: Provider::Kimi,
            key: Some("sk-kimi-x"),
            catalog: &catalog,
            model: &catalog[0],
        };
        let s = launch_settings(Target::Kimi, Some(&route), true).unwrap();
        assert_eq!(s["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "k3.1-pro");
        assert_eq!(
            s["env"]["ANTHROPIC_AUTH_TOKEN"],
            crate::relay::PLACEHOLDER_TOKEN
        );
        assert_eq!(s["modelPicker"]["options"][0]["label"], "K3.1 Pro");
        assert_eq!(
            s["availableModels"],
            json!(["opus", "sonnet", "haiku", "fable", "k3.1-pro"])
        );
        let direct = launch_settings(Target::Kimi, Some(&route), false).unwrap();
        assert_eq!(direct["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-kimi-x");

        let c = launch_settings(Target::Claude, None, true).unwrap();
        assert_eq!(c["env"]["ANTHROPIC_BASE_URL"], "");
        assert!(launch_settings(Target::Refuseless, None, true).is_none());
    }

    #[test]
    fn strips_only_clydes_keys_from_a_folder() {
        let mut root: Map<String, Value> = serde_json::from_value(json!({
            "permissions": {"allow": ["Bash(ls:*)"]},
            "env": {"ANTHROPIC_BASE_URL": "http://127.0.0.1:47311/kimi/", "MY_VAR": "1"},
            "availableModels": ["opus", "sonnet", "haiku", "fable", "k3.1-pro"]
        }))
        .unwrap();
        assert!(strip_pin(&mut root));
        assert_eq!(
            Value::Object(root),
            json!({"permissions": {"allow": ["Bash(ls:*)"]}, "env": {"MY_VAR": "1"}})
        );
    }

    #[test]
    fn a_provider_round_trip_leaves_the_users_settings_as_they_were() {
        let dir = std::env::temp_dir().join(format!("clyde-routing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let user = json!({
            "model": "opus",
            "env": {"CLAUDE_CODE_MAX_OUTPUT_TOKENS": "64000"},
            "permissions": {"allow": ["Bash(ls:*)"]}
        });
        std::fs::write(&path, user.to_string()).unwrap();
        let read =
            || -> Value { serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap() };

        let catalog = kimi_catalog();
        let route = ProviderRoute {
            provider: Provider::Kimi,
            key: Some("sk-kimi-x"),
            catalog: &catalog,
            model: &catalog[0],
        };
        let is_provider = |m: &str| m == "k3.1-pro";
        let mut prefs = Prefs::default();

        enter_provider_at(&path, &mut prefs, &route, true, is_provider).unwrap();
        let on = read();
        assert_eq!(on["env"]["ANTHROPIC_DEFAULT_OPUS_MODEL"], "k3.1-pro");
        assert_eq!(on["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], "64000");
        assert_eq!(on["modelPicker"]["options"][0]["model"], "k3.1-pro");
        assert_eq!(on["model"], "opus", "aliases follow the provider");
        assert_eq!(prefs.claude_model.as_deref(), Some("opus"));

        // `/model` in a session saves the provider's id as the default…
        let mut picked = read();
        picked["model"] = json!("k3.1-pro");
        std::fs::write(&path, picked.to_string()).unwrap();
        let (got, _) = reconcile_model_setting_at(&path, &mut prefs, is_provider).unwrap();
        assert_eq!(got.as_deref(), Some("k3.1-pro"));
        assert_eq!(
            read()["model"],
            "opus",
            "…and the user's own value comes back"
        );

        leave_provider_at(&path, &mut prefs, is_provider).unwrap();
        let off = read();
        assert_eq!(
            off["env"]["ANTHROPIC_BASE_URL"], "",
            "blanked for running sessions"
        );
        assert!(off.get("modelPicker").is_none());
        assert_eq!(off["model"], "opus");
        assert_eq!(off["availableModels"], json!(CLAUDE_FAMILIES));
        assert_eq!(off["permissions"], user["permissions"]);

        // Back on a provider: Clyde's guard goes.
        enter_provider_at(&path, &mut prefs, &route, true, is_provider).unwrap();
        assert!(read().get("availableModels").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn targets_round_trip() {
        for t in Target::ALL {
            assert_eq!(Target::parse(t.id()), Some(t));
        }
        assert_eq!(
            serde_json::to_value(Target::Refuseless).unwrap(),
            json!("refuseless")
        );
        assert!(command_script(Target::Kimi).contains(".claude/clyde/kimi.json"));
    }
}
