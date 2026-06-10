//! Parse Anthropic's `GET /api/oauth/usage` response into a [`UsageSnapshot`].
//!
//! This is the same endpoint Claude Code itself polls for its status line. The
//! response shape (reverse-engineered from the Claude Code binary, and verified
//! live) is:
//!
//! ```json
//! {
//!   "five_hour":  { "utilization": 18.0, "resets_at": "2026-06-10T18:50:00+00:00" },
//!   "seven_day":  { "utilization": 10.0, "resets_at": "2026-06-16T09:00:00+00:00" },
//!   "seven_day_opus":   null,
//!   "seven_day_sonnet": { "utilization": 0.0, "resets_at": null },
//!   "extra_usage": { ... }
//! }
//! ```
//!
//! `utilization` is already a 0..=100 percentage; `resets_at` is an RFC3339
//! timestamp (or null). Any window may be `null` when it doesn't apply to the
//! account's plan.

use serde_json::Value;

use crate::model::{now_ms, UsageSnapshot};

/// Build a usage snapshot from the parsed `/api/oauth/usage` JSON body. Returns
/// `None` if neither the 5-hour nor the 7-day window is present (e.g. an error
/// payload), so callers don't overwrite good data with an empty snapshot.
pub fn parse(body: &Value) -> Option<UsageSnapshot> {
    let five_hour = window(body, "five_hour");
    let seven_day = window(body, "seven_day");

    if five_hour.is_none() && seven_day.is_none() {
        return None;
    }

    let five_hour_utilization = five_hour.as_ref().and_then(|w| w.utilization);
    let seven_day_utilization = seven_day.as_ref().and_then(|w| w.utilization);

    // The UI shows the soonest upcoming reset across the windows that report one.
    let resets_at = [five_hour.as_ref(), seven_day.as_ref()]
        .iter()
        .filter_map(|w| w.and_then(|w| w.resets_at))
        .min();

    // `/api/oauth/usage` carries no allowed/rejected flag, so derive it: a window
    // at 100% utilization means that limit is exhausted.
    let limited = [five_hour_utilization, seven_day_utilization]
        .iter()
        .flatten()
        .any(|u| *u >= 100.0);
    let status = Some(if limited { "rejected" } else { "allowed" }.to_string());

    Some(UsageSnapshot {
        five_hour_utilization,
        seven_day_utilization,
        status,
        resets_at,
        updated_at: now_ms(),
    })
}

struct WindowUsage {
    utilization: Option<f64>,
    resets_at: Option<i64>,
}

fn window(body: &Value, key: &str) -> Option<WindowUsage> {
    let obj = body.get(key)?.as_object()?;
    Some(WindowUsage {
        utilization: obj.get("utilization").and_then(|v| v.as_f64()),
        resets_at: obj
            .get("resets_at")
            .and_then(|v| v.as_str())
            .and_then(parse_epoch),
    })
}

fn parse_epoch(s: &str) -> Option<i64> {
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_live_shape() {
        let body = json!({
            "five_hour": { "utilization": 18.0, "resets_at": "2026-06-10T18:50:00+00:00" },
            "seven_day": { "utilization": 10.0, "resets_at": "2026-06-16T09:00:00+00:00" },
            "seven_day_opus": null,
            "seven_day_sonnet": { "utilization": 0.0, "resets_at": null }
        });
        let snap = parse(&body).expect("should parse");
        assert_eq!(snap.five_hour_utilization, Some(18.0));
        assert_eq!(snap.seven_day_utilization, Some(10.0));
        assert_eq!(snap.status.as_deref(), Some("allowed"));
        // Soonest reset is the 5-hour window (2026-06-10T18:50:00Z).
        assert_eq!(snap.resets_at, Some(1_781_117_400));
    }

    #[test]
    fn marks_rejected_when_a_window_is_exhausted() {
        let body = json!({
            "five_hour": { "utilization": 100.0, "resets_at": null },
            "seven_day": { "utilization": 40.0, "resets_at": null }
        });
        let snap = parse(&body).unwrap();
        assert_eq!(snap.status.as_deref(), Some("rejected"));
    }

    #[test]
    fn none_when_no_known_window() {
        let body = json!({ "error": "nope" });
        assert!(parse(&body).is_none());
    }
}
