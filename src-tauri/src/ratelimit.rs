//! Parse Anthropic's `anthropic-ratelimit-unified-*` response headers into a
//! [`UsageSnapshot`]. These headers ride on every API response, so the proxy
//! gets per-account utilization for free — no extra polling.
//!
//! Header names are matched defensively (Anthropic has used both `5h`/`7d` and
//! `five_hour`/`seven_day` style window keys) so the parser keeps working if the
//! exact spelling shifts.

use http::HeaderMap;

use crate::model::{now_ms, UsageSnapshot};

const PREFIX: &str = "anthropic-ratelimit-unified-";

/// Build a usage snapshot from response headers. Returns `None` if the response
/// carried no recognizable unified rate-limit headers at all.
pub fn parse(headers: &HeaderMap) -> Option<UsageSnapshot> {
    let mut snap = UsageSnapshot {
        updated_at: now_ms(),
        ..Default::default()
    };
    let mut saw_any = false;

    for (name, value) in headers.iter() {
        let name = name.as_str();
        let Some(rest) = name.strip_prefix(PREFIX) else {
            continue;
        };
        let Ok(val) = value.to_str() else { continue };
        saw_any = true;

        match rest {
            "status" => snap.status = Some(val.to_string()),
            "reset" => snap.resets_at = parse_epoch(val),
            _ if rest.ends_with("-utilization") => {
                let window = rest.trim_end_matches("-utilization");
                if let Ok(pct) = val.parse::<f64>() {
                    match normalize_window(window) {
                        Window::FiveHour => snap.five_hour_utilization = Some(pct),
                        Window::SevenDay => snap.seven_day_utilization = Some(pct),
                        Window::Other => {}
                    }
                }
            }
            _ if rest.ends_with("-reset") => {
                // Per-window reset; prefer the soonest non-null one.
                if let Some(ts) = parse_epoch(val) {
                    snap.resets_at = Some(match snap.resets_at {
                        Some(existing) => existing.min(ts),
                        None => ts,
                    });
                }
            }
            _ => {}
        }
    }

    if saw_any {
        Some(snap)
    } else {
        None
    }
}

enum Window {
    FiveHour,
    SevenDay,
    Other,
}

fn normalize_window(w: &str) -> Window {
    match w {
        "5h" | "five_hour" | "fivehour" | "five-hour" => Window::FiveHour,
        "7d" | "seven_day" | "sevenday" | "seven-day" => Window::SevenDay,
        _ => Window::Other,
    }
}

fn parse_epoch(s: &str) -> Option<i64> {
    // Either a unix timestamp or an RFC3339 string.
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

/// True if a response status / headers indicate a hard rate-limit rejection
/// that should trigger failover.
pub fn is_rate_limited(status: http::StatusCode, headers: &HeaderMap) -> bool {
    if status == http::StatusCode::TOO_MANY_REQUESTS {
        return true;
    }
    headers
        .get("anthropic-ratelimit-unified-status")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("rejected"))
        .unwrap_or(false)
}
