//! Browser tools that survive an account switch, via Open Claude in Chrome.
//!
//! The official Claude-in-Chrome extension pairs with Claude Code through a
//! cloud rendezvous keyed on account uuid (see [`crate::chrome_link`]), so every
//! Clyde switch strands the browser. Open Claude in Chrome
//! (github.com/noemica-io/open-claude-in-chrome) reaches the browser a different
//! way: an MCP server talks to a native messaging host over a user-scoped unix
//! socket, and nothing on that path carries an account. Registered once as a
//! *user-scope* MCP server in Claude Code's global config — a file Clyde's
//! identity writes preserve key-for-key — it keeps working across every switch.
//!
//! **Clyde never ships its code.** The project is PolyForm Noncommercial, not
//! MIT, so Clyde fetches it from upstream onto the user's machine, pinned to the
//! commit that was reviewed ([`PINNED_COMMIT`]), and only wires it up.
//!
//! Loading the extension is the one step Clyde can't do: Chrome only accepts an
//! unpacked extension through `chrome://extensions` with Developer mode on.
//! Everything around that step — download, host registration, MCP registration —
//! happens here, and [`status`] reports each piece so the UI can show a
//! checklist instead of a wall of instructions.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::claude_sync;

/// Upstream repository.
pub const REPO_URL: &str = "https://github.com/noemica-io/open-claude-in-chrome";

/// The commit whose source was reviewed (2026-09-19: no outbound calls on the
/// default path, user-scoped socket). Moving this is a deliberate act — re-review
/// the diff first.
pub const PINNED_COMMIT: &str = "7544c3e98ee4ffb34bf1bd81d49093c3f3f973bb";

/// Name Claude Code shows the server under; tools become `mcp__open-claude-in-chrome__*`.
pub const MCP_NAME: &str = "open-claude-in-chrome";

/// Native messaging host name the extension connects to (fixed in its code).
const HOST_NAME: &str = "com.anthropic.open_claude_in_chrome";

/// Browsers the upstream installer supports on macOS: (label, profile root,
/// relative to `~/Library/Application Support`).
const BROWSERS: &[(&str, &str)] = &[
    ("Chrome", "Google/Chrome"),
    ("Edge", "Microsoft Edge"),
    ("Brave", "BraveSoftware/Brave-Browser"),
];

/// Everything the UI needs to render the setup checklist.
#[derive(Debug, Clone, Serialize, Default)]
pub struct OpenChromeStatus {
    /// Where the project lives (or will be downloaded to).
    pub repo_path: String,
    /// The project is on disk and looks like Open Claude in Chrome.
    pub downloaded: bool,
    /// The commit checked out, when downloaded.
    pub commit: Option<String>,
    /// That commit is the one Clyde reviewed ([`PINNED_COMMIT`]). False after a
    /// manual `git pull` — the code running in the browser is then unreviewed.
    pub reviewed: bool,
    /// `host/node_modules` exists.
    pub deps_installed: bool,
    /// Absolute path to `node`, when found. Setup can't proceed without it.
    pub node_path: Option<String>,
    /// The folder to pick in "Load unpacked".
    pub extension_dir: String,
    /// The id Chrome will assign that folder (derived from its path).
    pub expected_extension_id: Option<String>,
    /// Browser profiles that have loaded the extension, e.g. `"Chrome — Default"`.
    pub loaded_in: Vec<String>,
    /// Extension ids actually seen in those profiles.
    pub loaded_ids: Vec<String>,
    /// A native-host manifest points at our wrapper and admits every loaded id.
    pub host_registered: bool,
    /// Claude Code has the MCP server registered at user scope.
    pub mcp_registered: bool,
    /// The native host is running — i.e. the extension is live in a browser.
    pub connected: bool,
    /// Claude Code's own (account-bound) Chrome integration is enabled.
    pub builtin_enabled: bool,
    /// Every piece is wired: switching accounts can't break the browser tools.
    pub ready: bool,
}

// ---- paths ---------------------------------------------------------------

fn home() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow!("no home directory"))
}

fn app_support() -> Result<PathBuf> {
    Ok(home()?.join("Library/Application Support"))
}

/// Where Clyde downloads the project.
fn default_repo() -> Result<PathBuf> {
    Ok(home()?.join("Projects/Vendor/open-claude-in-chrome"))
}

/// The repo in use: whatever the registered MCP server points at, else the default.
fn repo_path() -> Result<PathBuf> {
    if let Some(server_js) = registered_server_script() {
        // <repo>/host/mcp-server.js → <repo>
        if let Some(repo) = Path::new(&server_js).parent().and_then(Path::parent) {
            return Ok(repo.to_path_buf());
        }
    }
    default_repo()
}

fn wrapper_path(repo: &Path) -> PathBuf {
    repo.join("host/native-host-wrapper.sh")
}

fn manifest_path(browser_root: &str) -> Result<PathBuf> {
    Ok(app_support()?
        .join(browser_root)
        .join("NativeMessagingHosts")
        .join(format!("{HOST_NAME}.json")))
}

/// `$TMPDIR/open-claude-in-chrome-<uid>/bridge.sock`, mirroring `host/endpoint.js`.
fn socket_path() -> Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(home()?)?.uid();
    Ok(std::env::temp_dir()
        .join(format!("open-claude-in-chrome-{uid}"))
        .join("bridge.sock"))
}

// ---- binaries ------------------------------------------------------------

/// Find an executable. A GUI app inherits launchd's bare PATH, not the user's
/// shell PATH, so look in the usual install locations before asking a login shell.
fn find_bin(name: &str) -> Option<PathBuf> {
    let home = home().ok()?;
    let candidates = [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".local/bin"),
        home.join(".volta/bin"),
        home.join(".bun/bin"),
        PathBuf::from("/usr/bin"),
    ];
    for dir in candidates {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    let out = Command::new("/bin/zsh")
        .args(["-lc", &format!("command -v {name}")])
        .output()
        .ok()?;
    let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && found.starts_with('/')).then(|| PathBuf::from(found))
}

/// Run a command, turning a non-zero exit into an error carrying its stderr.
/// `node_dir` is prepended to PATH so npm's `#!/usr/bin/env node` resolves.
fn run(cmd: &Path, args: &[&str], cwd: Option<&Path>, node_dir: Option<&Path>) -> Result<String> {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    if let Some(dir) = node_dir {
        let path = std::env::var("PATH").unwrap_or_default();
        c.env("PATH", format!("{}:{path}", dir.display()));
    }
    let out = c
        .output()
        .with_context(|| format!("couldn't run {}", cmd.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<&str> = stderr.lines().collect();
        let tail = lines[lines.len().saturating_sub(6)..].join("\n");
        bail!("{} failed: {tail}", cmd.display());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---- extension id --------------------------------------------------------

/// The id Chrome gives an unpacked extension: SHA-256 of its absolute
/// (symlink-resolved) path, first 16 bytes, each nibble mapped `0..f → a..p`.
/// Mirrors Chromium's `crx_file::id_util::GenerateIdForPath` on POSIX.
pub fn extension_id_for_path(path: &Path) -> Option<String> {
    let abs = std::fs::canonicalize(path).ok()?;
    Some(id_from_path_bytes(abs.to_string_lossy().as_bytes()))
}

fn id_from_path_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest[..16]
        .iter()
        .flat_map(|b| [b >> 4, b & 0x0f])
        .map(|n| (b'a' + n) as char)
        .collect()
}

/// Browser profiles whose preferences list an extension loaded from `ext_dir`,
/// with the id each assigned. Chrome records unpacked extensions under
/// `extensions.settings.<id>.path` in `Secure Preferences` (older builds:
/// `Preferences`).
fn loaded_profiles(ext_dir: &Path) -> Vec<(String, String)> {
    let Ok(target) = std::fs::canonicalize(ext_dir) else {
        return Vec::new();
    };
    let Ok(support) = app_support() else {
        return Vec::new();
    };

    let mut found = Vec::new();
    for (label, root) in BROWSERS {
        let Ok(entries) = std::fs::read_dir(support.join(root)) else {
            continue;
        };
        let mut profiles: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        profiles.sort();
        for profile in profiles.into_iter().filter(|p| p.is_dir()) {
            let name = profile
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            for file in ["Secure Preferences", "Preferences"] {
                if let Some(id) = extension_in_prefs(&profile.join(file), &target) {
                    found.push((format!("{label} — {name}"), id));
                    break;
                }
            }
        }
    }
    found
}

fn extension_in_prefs(prefs: &Path, target: &Path) -> Option<String> {
    let text = std::fs::read_to_string(prefs).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    extension_id_in_settings(&root, target)
}

/// The id of the entry in `extensions.settings` whose `path` is `target`.
/// A disabled extension still has an entry; `state == 0` marks it.
fn extension_id_in_settings(root: &Value, target: &Path) -> Option<String> {
    let settings = root.pointer("/extensions/settings")?.as_object()?;
    settings.iter().find_map(|(id, v)| {
        let path = v.get("path")?.as_str()?;
        let same = Path::new(path) == target
            || std::fs::canonicalize(path).ok().as_deref() == Some(target);
        let disabled = v.get("state").and_then(Value::as_i64) == Some(0);
        (same && !disabled).then(|| id.clone())
    })
}

// ---- status --------------------------------------------------------------

/// The `mcp-server.js` path of our registered server, if any.
fn registered_server_script() -> Option<String> {
    let root = claude_sync::read_global_config()?;
    server_script_in(&root)
}

fn server_script_in(root: &Value) -> Option<String> {
    let server = root.pointer(&format!("/mcpServers/{MCP_NAME}"))?;
    server
        .get("args")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|a| a.ends_with("mcp-server.js"))
        .map(str::to_string)
}

/// Whether some installed browser's manifest runs our wrapper and admits `ids`.
fn host_manifest_ok(repo: &Path, ids: &[String]) -> bool {
    let wrapper = wrapper_path(repo);
    if ids.is_empty() || !wrapper.is_file() {
        return false;
    }
    BROWSERS.iter().any(|(_, root)| {
        let Ok(path) = manifest_path(root) else {
            return false;
        };
        let Some(m) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            return false;
        };
        manifest_admits(&m, &wrapper, ids)
    })
}

fn manifest_admits(manifest: &Value, wrapper: &Path, ids: &[String]) -> bool {
    let points_at_us = manifest.get("path").and_then(Value::as_str) == wrapper.to_str();
    let origins: Vec<&str> = manifest
        .get("allowed_origins")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    points_at_us
        && ids
            .iter()
            .all(|id| origins.contains(&format!("chrome-extension://{id}/").as_str()))
}

fn socket_alive() -> bool {
    socket_path()
        .map(|p| std::os::unix::net::UnixStream::connect(p).is_ok())
        .unwrap_or(false)
}

fn head_commit(repo: &Path) -> Option<String> {
    let git = find_bin("git")?;
    run(&git, &["rev-parse", "HEAD"], Some(repo), None)
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn status() -> OpenChromeStatus {
    let Ok(repo) = repo_path() else {
        return OpenChromeStatus::default();
    };
    let ext_dir = repo.join("extension");
    let downloaded =
        repo.join("host/mcp-server.js").is_file() && ext_dir.join("manifest.json").is_file();

    let loaded = if downloaded {
        loaded_profiles(&ext_dir)
    } else {
        Vec::new()
    };
    let expected = extension_id_for_path(&ext_dir);
    let mut ids: Vec<String> = loaded.iter().map(|(_, id)| id.clone()).collect();
    ids.dedup();
    // Before anything is loaded, judge the manifest against the id Chrome will assign.
    let check_ids = if ids.is_empty() {
        expected.clone().into_iter().collect()
    } else {
        ids.clone()
    };

    let config = claude_sync::read_global_config();
    let commit = downloaded.then(|| head_commit(&repo)).flatten();
    let mut status = OpenChromeStatus {
        repo_path: repo.display().to_string(),
        downloaded,
        reviewed: commit.as_deref() == Some(PINNED_COMMIT),
        commit,
        deps_installed: repo.join("host/node_modules").is_dir(),
        node_path: find_bin("node").map(|p| p.display().to_string()),
        extension_dir: ext_dir.display().to_string(),
        expected_extension_id: expected,
        loaded_in: loaded.into_iter().map(|(name, _)| name).collect(),
        loaded_ids: ids,
        host_registered: downloaded && host_manifest_ok(&repo, &check_ids),
        mcp_registered: config.as_ref().and_then(server_script_in).is_some(),
        connected: socket_alive(),
        builtin_enabled: config
            .as_ref()
            .and_then(|c| c.get("claudeInChromeDefaultEnabled"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        ready: false,
    };
    status.ready = status.host_registered && status.mcp_registered && !status.loaded_in.is_empty();
    status
}

// ---- setup / teardown ----------------------------------------------------

/// Download (if needed), install host dependencies, register the native host
/// and the MCP server. Idempotent: re-running after the extension is loaded
/// re-writes the host manifest with the id Chrome actually assigned.
pub fn setup() -> Result<OpenChromeStatus> {
    let repo = repo_path()?;
    let node = find_bin("node").ok_or_else(|| {
        anyhow!("Node.js wasn't found. Install it (brew install node), then try again.")
    })?;
    let node_dir = node.parent().map(Path::to_path_buf);

    if !repo.join("host/mcp-server.js").is_file() {
        download(&repo)?;
    }

    if !repo.join("host/node_modules").is_dir() {
        let npm = find_bin("npm").ok_or_else(|| anyhow!("npm wasn't found next to Node.js"))?;
        run(
            &npm,
            // --ignore-scripts: the three runtime deps need no install hooks, and
            // refusing them shuts the usual npm supply-chain door.
            &[
                "ci",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--omit=dev",
            ],
            Some(&repo.join("host")),
            node_dir.as_deref(),
        )
        .context("installing the helper's dependencies")?;
    }

    write_wrapper(&repo, &node)?;

    let ext_dir = repo.join("extension");
    let mut ids: Vec<String> = loaded_profiles(&ext_dir)
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    if let Some(expected) = extension_id_for_path(&ext_dir) {
        ids.push(expected);
    }
    ids.sort();
    ids.dedup();
    write_manifests(&repo, &ids)?;

    register_mcp(&repo, &node)?;
    Ok(status())
}

fn download(repo: &Path) -> Result<()> {
    let git = find_bin("git").ok_or_else(|| anyhow!("git wasn't found"))?;
    if repo.exists() {
        bail!(
            "{} exists but isn't Open Claude in Chrome — move it aside and try again",
            repo.display()
        );
    }
    if let Some(parent) = repo.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let repo_s = repo.to_string_lossy();
    run(&git, &["clone", "--quiet", REPO_URL, &repo_s], None, None)
        .context("downloading Open Claude in Chrome")?;
    run(
        &git,
        &[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--quiet",
            PINNED_COMMIT,
        ],
        Some(repo),
        None,
    )
    .context("checking out the reviewed version")?;
    if head_commit(repo).as_deref() != Some(PINNED_COMMIT) {
        bail!("the download isn't at the reviewed version — not wiring it up");
    }
    Ok(())
}

/// Chrome launches native hosts with a bare environment, so the wrapper pins
/// the absolute node path — same shape as upstream's `install.sh` writes.
fn write_wrapper(repo: &Path, node: &Path) -> Result<()> {
    let wrapper = wrapper_path(repo);
    let script = format!(
        "#!/bin/sh\nexec \"{}\" \"{}\"\n",
        node.display(),
        repo.join("host/native-host.js").display()
    );
    std::fs::write(&wrapper, script).context("writing the native host wrapper")?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn manifest_json(wrapper: &Path, ids: &[String]) -> Value {
    json!({
        "name": HOST_NAME,
        "description": "Open Claude in Chrome Native Messaging Host",
        "path": wrapper.display().to_string(),
        "type": "stdio",
        "allowed_origins": ids
            .iter()
            .map(|id| format!("chrome-extension://{id}/"))
            .collect::<Vec<_>>(),
    })
}

/// One manifest per installed browser (skipped when the browser isn't there).
fn write_manifests(repo: &Path, ids: &[String]) -> Result<()> {
    let body = serde_json::to_string_pretty(&manifest_json(&wrapper_path(repo), ids))?;
    let support = app_support()?;
    let mut wrote = false;
    for (_, root) in BROWSERS {
        if !support.join(root).is_dir() {
            continue;
        }
        let path = manifest_path(root)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
        wrote = true;
    }
    if !wrote {
        bail!("no Chrome, Edge or Brave profile folder found");
    }
    Ok(())
}

/// Register through Claude Code's own CLI rather than editing its config file:
/// it owns that file's format and writes it while sessions are running.
fn register_mcp(repo: &Path, node: &Path) -> Result<()> {
    let claude = find_bin("claude").ok_or_else(|| anyhow!("the claude CLI wasn't found"))?;
    let server = repo.join("host/mcp-server.js");
    let want = server.display().to_string();
    if registered_server_script().as_deref() == Some(want.as_str()) {
        return Ok(());
    }
    // A stale entry (moved repo) would make `add` fail; clear it first.
    let _ = run(
        &claude,
        &["mcp", "remove", "--scope", "user", MCP_NAME],
        None,
        None,
    );
    run(
        &claude,
        &[
            "mcp",
            "add",
            "--scope",
            "user",
            MCP_NAME,
            "--",
            &node.display().to_string(),
            &want,
        ],
        None,
        None,
    )
    .context("registering the browser tools with Claude Code")?;
    Ok(())
}

/// Undo the wiring: MCP registration and our host manifests. The downloaded
/// project stays on disk; the extension is removed from `chrome://extensions`.
pub fn remove() -> Result<OpenChromeStatus> {
    let repo = repo_path()?;
    if let Some(claude) = find_bin("claude") {
        let _ = run(
            &claude,
            &["mcp", "remove", "--scope", "user", MCP_NAME],
            None,
            None,
        );
    }
    let wrapper = wrapper_path(&repo);
    for (_, root) in BROWSERS {
        let path = manifest_path(root)?;
        let ours = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|m| m.get("path").and_then(Value::as_str) == wrapper.to_str());
        if ours {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(status())
}

/// Turn Claude Code's built-in (account-bound) Chrome integration on or off, so
/// sessions don't see two sets of browser tools.
pub fn set_builtin(enabled: bool) -> Result<OpenChromeStatus> {
    claude_sync::set_global_flag("claudeInChromeDefaultEnabled", enabled)?;
    Ok(status())
}

/// Show the extension folder in Finder, ready to pick in "Load unpacked".
pub fn reveal_extension() -> Result<()> {
    let dir = repo_path()?.join("extension");
    if !dir.is_dir() {
        bail!("download Open Claude in Chrome first");
    }
    Command::new("open").arg("-R").arg(&dir).spawn()?;
    Ok(())
}

/// Open `chrome://extensions` (a URL scheme only Chrome itself will open).
pub fn open_extensions_page() -> Result<()> {
    Command::new("open")
        .args(["-a", "Google Chrome", "chrome://extensions"])
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_id_is_sha256_nibbles_mapped_to_a_through_p() {
        let id = id_from_path_bytes(b"/tmp/ext");
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| (b'a'..=b'p').contains(&b)));
        // First byte of sha256("/tmp/ext") drives the first two letters.
        let d = Sha256::digest(b"/tmp/ext");
        assert_eq!(id.as_bytes()[0], b'a' + (d[0] >> 4));
        assert_eq!(id.as_bytes()[1], b'a' + (d[0] & 0x0f));
    }

    #[test]
    fn finds_the_registered_server_script() {
        let cfg = json!({"mcpServers": {MCP_NAME: {
            "type": "stdio", "command": "/opt/homebrew/bin/node",
            "args": ["/Users/x/Projects/Vendor/open-claude-in-chrome/host/mcp-server.js"]
        }}});
        assert_eq!(
            server_script_in(&cfg).as_deref(),
            Some("/Users/x/Projects/Vendor/open-claude-in-chrome/host/mcp-server.js")
        );
        assert!(server_script_in(&json!({"mcpServers": {}})).is_none());
    }

    #[test]
    fn manifest_must_run_our_wrapper_and_admit_every_id() {
        let wrapper = Path::new("/r/host/native-host-wrapper.sh");
        let ids = vec!["a".repeat(32), "b".repeat(32)];
        let m = manifest_json(wrapper, &ids);
        assert!(manifest_admits(&m, wrapper, &ids));
        assert!(!manifest_admits(&m, Path::new("/elsewhere.sh"), &ids));
        assert!(!manifest_admits(&m, wrapper, &["c".repeat(32)]));
    }

    #[test]
    fn reads_the_id_of_an_enabled_unpacked_extension() {
        let target = Path::new("/r/extension");
        let prefs = json!({"extensions": {"settings": {
            "aaaa": {"path": "/somewhere/else", "location": 4},
            "bbbb": {"path": "/r/extension", "location": 4, "state": 1}
        }}});
        assert_eq!(
            extension_id_in_settings(&prefs, target).as_deref(),
            Some("bbbb")
        );

        let disabled = json!({"extensions": {"settings": {
            "bbbb": {"path": "/r/extension", "location": 4, "state": 0}
        }}});
        assert!(extension_id_in_settings(&disabled, target).is_none());
    }
}
