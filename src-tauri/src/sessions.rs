//! Running `claude` sessions: which folder each is in and how long it's been
//! up. Read with standard tools (`pgrep`, `ps`, `lsof`); nothing is attached to
//! or injected into the processes.

use std::process::Command;

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Session {
    pub pid: u32,
    /// Working directory, when `lsof` could read it.
    pub cwd: Option<String>,
    /// Seconds since the process started.
    pub running_secs: Option<u64>,
    /// `CLAUDE_CONFIG_DIR` the session was started with, if any — a session with
    /// its own config dir isn't affected by switching in Clyde.
    pub config_dir: Option<String>,
}

pub fn list() -> Vec<Session> {
    let Ok(out) = Command::new("pgrep").args(["-x", "claude"]).output() else {
        return Vec::new();
    };
    let mut sessions: Vec<Session> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .map(|pid| Session {
            pid,
            cwd: cwd(pid),
            running_secs: elapsed(pid),
            config_dir: config_dir(pid),
        })
        .collect();
    sessions.sort_by_key(|s| s.running_secs.unwrap_or(0));
    sessions
}

fn cwd(pid: u32) -> Option<String> {
    let out = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix('n').map(str::to_string))
}

fn elapsed(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "etime=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    parse_etime(String::from_utf8_lossy(&out.stdout).trim())
}

/// `ps` elapsed time: `[[dd-]hh:]mm:ss`.
fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [m, s] => (0, *m, *s),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    Some(days * 86_400 + h * 3_600 + m * 60 + sec)
}

/// macOS `ps -E` prints the environment after the command for the user's own
/// processes.
fn config_dir(pid: u32) -> Option<String> {
    let out = Command::new("ps")
        .args(["-E", "-ww", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .find_map(|w| w.strip_prefix("CLAUDE_CONFIG_DIR="))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::parse_etime;

    #[test]
    fn parses_ps_elapsed_formats() {
        assert_eq!(parse_etime("05:07"), Some(307));
        assert_eq!(parse_etime("02:05:07"), Some(7_507));
        assert_eq!(parse_etime("3-02:05:07"), Some(3 * 86_400 + 7_507));
        assert_eq!(parse_etime("garbage"), None);
    }
}
