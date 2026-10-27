//! Browser tools that survive an account switch, via Playwright MCP.
//!
//! The official Claude-in-Chrome extension pairs with Claude Code through a
//! cloud rendezvous keyed on account uuid (see [`crate::chrome_link`]), so every
//! Clyde switch strands the browser. Microsoft's Playwright MCP reaches Chrome a
//! different way: its "Playwright Extension" (Chrome Web Store) accepts a local
//! connection from the MCP server, authenticated by a per-profile token, and
//! nothing on that path carries a Claude account. Registered once as a
//! *user-scope* MCP server in Claude Code's global config — a file Clyde's
//! identity writes preserve key-for-key — it keeps working across every switch.
//!
//! Installing the extension is the one step Clyde can't do (the Web Store needs
//! a click). Everything else is automatic: Clyde finds which Chrome profiles
//! have it, reads the connection token straight out of that profile's storage,
//! and registers the server with Claude Code.
//!
//! This replaces Open Claude in Chrome (Clyde ≤ 1.0), which stalled upstream
//! and was licensed for noncommercial use only; [`remove_legacy`] cleans it up.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::claude_sync;

/// Chrome Web Store id of Microsoft's "Playwright Extension".
pub const EXTENSION_ID: &str = "mmlmfjhmonkocbjadbfplnigmagldckm";

/// Where to install it.
pub const STORE_URL: &str =
    "https://chromewebstore.google.com/detail/playwright-extension/mmlmfjhmonkocbjadbfplnigmagldckm";

/// The `@playwright/mcp` release Clyde registers. Pinned so a session never
/// runs a version nobody tried; tested 2026-10-28 (navigate, click, type,
/// scroll, screenshot) from sessions on two different Claude accounts.
pub const MCP_VERSION: &str = "0.0.83";

/// Name Claude Code shows the server under; tools become `mcp__playwright__*`.
pub const MCP_NAME: &str = "playwright";

/// The environment variable that lets the server skip Chrome's approval dialog.
const TOKEN_ENV: &str = "PLAYWRIGHT_MCP_EXTENSION_TOKEN";

/// Clyde ≤ 1.0's browser tools, for cleanup.
const LEGACY_MCP_NAME: &str = "open-claude-in-chrome";
const LEGACY_HOST: &str = "com.anthropic.open_claude_in_chrome";

/// Browsers the legacy host manifests were written for, relative to
/// `~/Library/Application Support`.
const LEGACY_BROWSER_ROOTS: &[&str] = &[
    "Google/Chrome",
    "Microsoft Edge",
    "BraveSoftware/Brave-Browser",
];

/// One Chrome profile, as the setup screen lists it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChromeProfile {
    /// Directory name, e.g. `"Default"` or `"Profile 2"` — what the server's
    /// `--profile-dir-name` takes.
    pub dir: String,
    /// The name Chrome shows for it.
    pub name: String,
    /// The Google account signed into it, if any.
    pub email: Option<String>,
    /// The Playwright Extension is installed in it.
    pub has_extension: bool,
    /// Clyde could read the extension's connection token from it.
    pub token_found: bool,
}

/// Everything the UI needs to render the setup checklist.
#[derive(Debug, Clone, Serialize, Default)]
pub struct BrowserStatus {
    /// `npx` was found; the server can't start without Node.js.
    pub node_found: bool,
    /// Every Chrome profile, in Chrome's order.
    pub profiles: Vec<ChromeProfile>,
    /// The profile the registered server connects to.
    pub profile: Option<String>,
    /// Claude Code has the server registered at user scope.
    pub mcp_registered: bool,
    /// The registered server carries a token (no approval dialog).
    pub token_set: bool,
    /// The registered token differs from the one now in the profile — the
    /// extension was reinstalled, so connections would be refused.
    pub token_stale: bool,
    /// The `@playwright/mcp` version the registration runs.
    pub registered_version: Option<String>,
    /// The version Clyde would register today.
    pub pinned_version: String,
    /// Claude Code's own (account-bound) Chrome integration is enabled.
    pub builtin_enabled: bool,
    /// Pieces of Open Claude in Chrome are still wired up.
    pub legacy_present: bool,
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

fn chrome_root() -> Result<PathBuf> {
    Ok(app_support()?.join("Google/Chrome"))
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

/// PATH for child processes: the directory holding `bin` first, so npx's
/// `#!/usr/bin/env node` resolves to the same Node install.
fn path_with(bin: &Path) -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    match bin.parent() {
        Some(dir) => format!("{}:{path}", dir.display()),
        None => path,
    }
}

/// Run a command, turning a non-zero exit into an error carrying its stderr.
fn run(cmd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .env("PATH", path_with(cmd))
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

// ---- Chrome profiles -----------------------------------------------------

/// Chrome's profiles from `Local State`, Default first then by directory.
fn chrome_profiles() -> Vec<ChromeProfile> {
    let Ok(root) = chrome_root() else {
        return Vec::new();
    };
    let local_state = std::fs::read_to_string(root.join("Local State"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let mut profiles: Vec<ChromeProfile> = local_state
        .as_ref()
        .and_then(|v| v.pointer("/profile/info_cache"))
        .and_then(Value::as_object)
        .map(|cache| {
            cache
                .iter()
                .map(|(dir, info)| profile_entry(&root, dir, info))
                .collect()
        })
        .unwrap_or_default();
    profiles.sort_by(|a, b| {
        (a.dir != "Default", natural_key(&a.dir)).cmp(&(b.dir != "Default", natural_key(&b.dir)))
    });
    profiles
}

/// "Profile 10" sorts after "Profile 9".
fn natural_key(dir: &str) -> (String, u32) {
    let (word, num) = dir.rsplit_once(' ').unwrap_or((dir, "0"));
    (word.to_string(), num.parse().unwrap_or(0))
}

fn profile_entry(root: &Path, dir: &str, info: &Value) -> ChromeProfile {
    let path = root.join(dir);
    let str_of = |k: &str| {
        info.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    ChromeProfile {
        dir: dir.to_string(),
        name: str_of("name").unwrap_or_else(|| dir.to_string()),
        email: str_of("user_name"),
        has_extension: path.join("Extensions").join(EXTENSION_ID).is_dir(),
        token_found: read_token(&path).is_some(),
    }
}

// ---- the extension's token -----------------------------------------------

/// The extension keeps its connection token in its own page's `localStorage`
/// (`auth-token`), which Chrome stores in the profile's `Local Storage`
/// LevelDB. Recent writes sit uncompressed in the `.log` journal and usually in
/// the `.ldb` tables too, so a byte scan finds them without a LevelDB reader.
/// Newest file wins; within a file, the last occurrence wins.
pub fn read_token(profile_dir: &Path) -> Option<String> {
    let dir = profile_dir.join("Local Storage/leveldb");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("log") | Some("ldb")
            )
        })
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .iter()
        .find_map(|(_, p)| token_in_bytes(&std::fs::read(p).ok()?))
}

fn token_in_bytes(bytes: &[u8]) -> Option<String> {
    let key = format!("_chrome-extension://{EXTENSION_ID}\u{0}\u{1}auth-token");
    let key = key.as_bytes();
    let mut found = None;
    let mut from = 0;
    while let Some(at) = find(&bytes[from..], key) {
        let start = from + at + key.len();
        // The value follows within a few bytes of record framing, as a
        // Latin-1 string (`0x01` prefix) of base64url characters.
        // Framing can contain 0x01 bytes too, so try each one.
        let window = &bytes[start..bytes.len().min(start + 24)];
        let value = window
            .iter()
            .enumerate()
            .filter(|(_, &b)| b == 1)
            .map(|(rel, _)| {
                bytes[start + rel + 1..]
                    .iter()
                    .take_while(|b| b.is_ascii_alphanumeric() || **b == b'-' || **b == b'_')
                    .map(|&b| b as char)
                    .collect::<String>()
            })
            .find(|v| (20..=128).contains(&v.len()));
        if value.is_some() {
            found = value;
        }
        from = start;
    }
    found
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ---- registration --------------------------------------------------------

/// What Claude Code has registered under [`MCP_NAME`], if anything.
struct Registered {
    profile: Option<String>,
    token: Option<String>,
    version: Option<String>,
}

fn registered(config: &Value) -> Option<Registered> {
    let server = config.pointer(&format!("/mcpServers/{MCP_NAME}"))?;
    let args: Vec<&str> = server
        .get("args")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // Only count it as ours when it runs the extension bridge; a plain
    // Playwright server under the same name launches its own browser.
    if !args.contains(&"--extension") {
        return None;
    }
    let profile = args
        .iter()
        .position(|a| *a == "--profile-dir-name")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.to_string());
    let version = args
        .iter()
        .find_map(|a| a.strip_prefix("@playwright/mcp@"))
        .map(str::to_string);
    let token = server
        .pointer(&format!("/env/{TOKEN_ENV}"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some(Registered {
        profile,
        token,
        version,
    })
}

fn legacy_present(config: Option<&Value>) -> bool {
    let mcp = config
        .and_then(|c| c.pointer(&format!("/mcpServers/{LEGACY_MCP_NAME}")))
        .is_some();
    mcp || legacy_manifests().iter().any(|p| p.is_file())
}

fn legacy_manifests() -> Vec<PathBuf> {
    let Ok(support) = app_support() else {
        return Vec::new();
    };
    LEGACY_BROWSER_ROOTS
        .iter()
        .map(|root| {
            support
                .join(root)
                .join("NativeMessagingHosts")
                .join(format!("{LEGACY_HOST}.json"))
        })
        .collect()
}

pub fn status() -> BrowserStatus {
    let config = claude_sync::read_global_config();
    let reg = config.as_ref().and_then(registered);
    let profiles = chrome_profiles();
    let profile = reg.as_ref().and_then(|r| r.profile.clone());
    let current_token = profile
        .as_deref()
        .and_then(|dir| chrome_root().ok().map(|r| r.join(dir)))
        .and_then(|p| read_token(&p));

    let mut s = BrowserStatus {
        node_found: find_bin("npx").is_some(),
        profile: profile.clone(),
        mcp_registered: reg.is_some(),
        token_set: reg.as_ref().is_some_and(|r| r.token.is_some()),
        token_stale: match (&reg, &current_token) {
            (Some(Registered { token: Some(t), .. }), Some(now)) => t != now,
            _ => false,
        },
        registered_version: reg.as_ref().and_then(|r| r.version.clone()),
        pinned_version: MCP_VERSION.to_string(),
        builtin_enabled: config
            .as_ref()
            .and_then(|c| c.get("claudeInChromeDefaultEnabled"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        legacy_present: legacy_present(config.as_ref()),
        ready: false,
        profiles,
    };
    let profile_has_ext = s
        .profiles
        .iter()
        .any(|p| Some(&p.dir) == profile.as_ref() && p.has_extension);
    s.ready = s.mcp_registered && s.token_set && !s.token_stale && profile_has_ext;
    s
}

// ---- actions -------------------------------------------------------------

/// Open the extension's Web Store page in `profile`, ready for "Add to Chrome".
pub fn open_store(profile: &str) -> Result<()> {
    open_in_chrome(profile, STORE_URL)
}

/// Open `chrome://extensions` in `profile` (to remove the legacy extension).
pub fn open_extensions_page(profile: &str) -> Result<()> {
    open_in_chrome(profile, "chrome://extensions")
}

fn open_in_chrome(profile: &str, url: &str) -> Result<()> {
    Command::new("open")
        .args(["-na", "Google Chrome", "--args"])
        .arg(format!("--profile-directory={profile}"))
        .arg(url)
        .spawn()
        .context("couldn't open Google Chrome")?;
    Ok(())
}

/// Register the server with Claude Code for `profile`. The token comes from
/// the profile's own storage unless one is passed (the paste fallback). Also
/// clears out Clyde 1.0's Open Claude in Chrome wiring.
pub fn setup(profile: &str, token: Option<String>) -> Result<BrowserStatus> {
    let profiles = chrome_profiles();
    let Some(p) = profiles.iter().find(|p| p.dir == profile) else {
        bail!("Chrome has no profile named {profile}");
    };
    if !p.has_extension {
        bail!("the Playwright Extension isn't installed in {} yet", p.name);
    }
    let token = match token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) => {
            // Accept a pasted `NAME=value` line as well as the bare value.
            let t = t.strip_prefix(&format!("{TOKEN_ENV}=")).unwrap_or(&t);
            t.trim_matches('"').to_string()
        }
        None => read_token(&chrome_root()?.join(profile)).ok_or_else(|| {
            anyhow!(
                "couldn't find the extension's token yet — click the Playwright icon in Chrome's toolbar once, then try again (or paste the token)"
            )
        })?,
    };
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("that doesn't look like a Playwright token");
    }

    let claude = find_bin("claude").ok_or_else(|| anyhow!("the claude CLI wasn't found"))?;
    let npx = find_bin("npx").ok_or_else(|| {
        anyhow!("Node.js wasn't found. Install it (brew install node), then try again.")
    })?;

    // `add` refuses an existing name, so clear ours first. Register through
    // Claude Code's own CLI rather than editing its config file: it owns that
    // file's format and writes it while sessions are running.
    let _ = run(&claude, &["mcp", "remove", "--scope", "user", MCP_NAME]);
    let env = format!("{TOKEN_ENV}={token}");
    let package = format!("@playwright/mcp@{MCP_VERSION}");
    let npx_s = npx.display().to_string();
    run(
        &claude,
        &[
            "mcp",
            "add",
            "--scope",
            "user",
            MCP_NAME,
            "-e",
            &env,
            "--",
            &npx_s,
            "-y",
            &package,
            "--extension",
            "--profile-dir-name",
            profile,
        ],
    )
    .context("registering the browser tools with Claude Code")?;

    remove_legacy()?;
    Ok(status())
}

/// Unregister the server. The extension stays in Chrome.
pub fn remove() -> Result<BrowserStatus> {
    if let Some(claude) = find_bin("claude") {
        let _ = run(&claude, &["mcp", "remove", "--scope", "user", MCP_NAME]);
    }
    Ok(status())
}

/// Undo Clyde 1.0's Open Claude in Chrome wiring: its MCP registration and its
/// native-host manifests. The unpacked extension itself can only be removed
/// from `chrome://extensions`.
pub fn remove_legacy() -> Result<()> {
    let config = claude_sync::read_global_config();
    let has_mcp = config
        .as_ref()
        .and_then(|c| c.pointer(&format!("/mcpServers/{LEGACY_MCP_NAME}")))
        .is_some();
    if has_mcp {
        if let Some(claude) = find_bin("claude") {
            run(
                &claude,
                &["mcp", "remove", "--scope", "user", LEGACY_MCP_NAME],
            )
            .context("removing Open Claude in Chrome from Claude Code")?;
        }
    }
    for path in legacy_manifests() {
        // Only ever a manifest that runs Open Claude in Chrome's host.
        let ours = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|m| m.get("path").and_then(Value::as_str).map(str::to_string))
            .is_some_and(|p| p.contains("open-claude-in-chrome"));
        if ours {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(())
}

/// Turn Claude Code's built-in (account-bound) Chrome integration on or off, so
/// sessions don't see two sets of browser tools.
pub fn set_builtin(enabled: bool) -> Result<BrowserStatus> {
    claude_sync::set_global_flag("claudeInChromeDefaultEnabled", enabled)?;
    Ok(status())
}

// ---- live test -----------------------------------------------------------

/// Result of [`test`]: the browser answered through the registered server.
#[derive(Debug, Clone, Serialize)]
pub struct BrowserTest {
    pub ok: bool,
    pub detail: String,
}

/// Start the registered server exactly as Claude Code would, connect it to
/// Chrome and list the tabs it can see. Proves the whole path end to end
/// without involving any Claude account.
pub fn test() -> Result<BrowserTest> {
    let config =
        claude_sync::read_global_config().ok_or_else(|| anyhow!("no Claude Code config"))?;
    let server = config
        .pointer(&format!("/mcpServers/{MCP_NAME}"))
        .ok_or_else(|| anyhow!("set up the browser tools first"))?;
    let command = server
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("the registration has no command"))?;
    let args: Vec<String> = server
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let mut cmd = Command::new(command);
    cmd.args(&args)
        .env("PATH", path_with(Path::new(command)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(env) = server.get("env").and_then(Value::as_object) {
        for (k, v) in env {
            if let Some(v) = v.as_str() {
                cmd.env(k, v);
            }
        }
    }
    let mut child = cmd
        .spawn()
        .context("couldn't start the Playwright server")?;
    let result = talk(&mut child);
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Minimal MCP over stdio: initialize, then `browser_tabs` → list.
fn talk(child: &mut std::process::Child) -> Result<BrowserTest> {
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut send = |v: Value| -> Result<()> {
        writeln!(stdin, "{v}")?;
        stdin.flush()?;
        Ok(())
    };
    // The first run downloads the package, so allow for it.
    let deadline = Instant::now() + Duration::from_secs(90);
    let wait_for = |id: i64| -> Result<Value> {
        loop {
            let left = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| anyhow!("Chrome didn't answer in time — is it open?"))?;
            let line = rx
                .recv_timeout(left)
                .map_err(|_| anyhow!("Chrome didn't answer in time — is it open?"))?;
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v.get("id").and_then(Value::as_i64) == Some(id) {
                    return Ok(v);
                }
            }
        }
    };

    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "clyde", "version": env!("CARGO_PKG_VERSION")}
        }}),
    )?;
    wait_for(1)?;
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
    send(
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "browser_tabs", "arguments": {"action": "list"}
        }}),
    )?;
    let reply = wait_for(2)?;
    Ok(read_test_reply(&reply))
}

fn read_test_reply(reply: &Value) -> BrowserTest {
    if let Some(e) = reply.get("error") {
        let msg = e.get("message").and_then(Value::as_str).unwrap_or("error");
        return BrowserTest {
            ok: false,
            detail: msg.to_string(),
        };
    }
    let result = reply.get("result").cloned().unwrap_or_default();
    let text: String = result
        .get("content")
        .and_then(Value::as_array)
        .map(|c| {
            c.iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let failed = result.get("isError").and_then(Value::as_bool) == Some(true);
    if failed {
        // Skip the markdown heading ("### Error") the server puts first.
        let first = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or("the browser didn't answer");
        return BrowserTest {
            ok: false,
            detail: first.to_string(),
        };
    }
    BrowserTest {
        ok: true,
        detail: "Chrome answered — the browser tools work with every account.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(value: &[u8]) -> Vec<u8> {
        let mut b = b"junk".to_vec();
        b.extend_from_slice(format!("_chrome-extension://{EXTENSION_ID}").as_bytes());
        b.extend_from_slice(b"\x00\x01auth-token");
        b.extend_from_slice(b"\x01\x9aDK\x00\x00\x00\x00");
        b.push(1);
        b.extend_from_slice(value);
        b.extend_from_slice(b"\x00more");
        b
    }

    /// Against this Mac's real Chrome: `cargo test -- --ignored live_status`.
    #[test]
    #[ignore]
    fn live_status() {
        let s = status();
        for p in &s.profiles {
            eprintln!(
                "{} ({}): extension={} token={}",
                p.dir, p.name, p.has_extension, p.token_found
            );
        }
        eprintln!(
            "registered={} profile={:?} token_set={} stale={} version={:?} legacy={} ready={}",
            s.mcp_registered,
            s.profile,
            s.token_set,
            s.token_stale,
            s.registered_version,
            s.legacy_present,
            s.ready
        );
    }

    /// Drives the real Chrome: `cargo test -- --ignored live_test`.
    #[test]
    #[ignore]
    fn live_test() {
        eprintln!("{:?}", test());
    }

    #[test]
    fn reads_the_token_out_of_leveldb_bytes() {
        let tok = "hinlo3v3c29-q7r74bc7WhtckvmcA-BCSZYkHIn_op8";
        assert_eq!(
            token_in_bytes(&record(tok.as_bytes())).as_deref(),
            Some(tok)
        );
        // The later of two writes wins.
        let mut two = record(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        two.extend(record(tok.as_bytes()));
        assert_eq!(token_in_bytes(&two).as_deref(), Some(tok));
        assert!(token_in_bytes(b"nothing here").is_none());
        assert!(token_in_bytes(&record(b"short")).is_none());
    }

    #[test]
    fn recognises_only_extension_mode_registrations() {
        let cfg = json!({"mcpServers": {MCP_NAME: {
            "type": "stdio", "command": "/opt/homebrew/bin/npx",
            "args": ["-y", "@playwright/mcp@0.0.83", "--extension", "--profile-dir-name", "Profile 2"],
            "env": {TOKEN_ENV: "abc"}
        }}});
        let r = registered(&cfg).unwrap();
        assert_eq!(r.profile.as_deref(), Some("Profile 2"));
        assert_eq!(r.version.as_deref(), Some("0.0.83"));
        assert_eq!(r.token.as_deref(), Some("abc"));

        let plain = json!({"mcpServers": {MCP_NAME: {
            "command": "npx", "args": ["@playwright/mcp@latest"]
        }}});
        assert!(registered(&plain).is_none());
    }

    #[test]
    fn profiles_sort_default_first_then_numerically() {
        let mut dirs = ["Profile 10", "Profile 2", "Default"];
        dirs.sort_by_key(|d| (*d != "Default", natural_key(d)));
        assert_eq!(dirs, ["Default", "Profile 2", "Profile 10"]);
    }

    #[test]
    fn reports_tool_errors_from_the_server() {
        let bad = json!({"id": 2, "result": {"isError": true, "content": [
            {"type": "text", "text": "### Error\nExtension connection timeout."}
        ]}});
        let t = read_test_reply(&bad);
        assert!(!t.ok);
        assert_eq!(t.detail, "Extension connection timeout.");
        let good = json!({"id": 2, "result": {"content": [{"type": "text", "text": "- 0: (current) [](about:blank)"}]}});
        assert!(read_test_reply(&good).ok);
    }
}
