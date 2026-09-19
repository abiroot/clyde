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
//!
//! Newer responses also carry a `limits` array — the source of truth for caps
//! that aren't the plain session/weekly pair, most importantly per-model weekly
//! limits (verified live 2026-09-19, Claude Code 2.1.278):
//!
//! ```json
//! "limits": [
//!   { "kind": "session",        "percent": 9,  "severity": "normal",   "resets_at": "…", "scope": null },
//!   { "kind": "weekly_all",     "percent": 77, "severity": "warning",  "resets_at": "…", "scope": null },
//!   { "kind": "weekly_scoped",  "percent": 94, "severity": "critical", "resets_at": "…",
//!     "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null } }
//! ]
//! ```
//!
//! The model-scoped weekly cap exists *only* there (`seven_day_opus` etc. are
//! null), so reading just `five_hour` / `seven_day` silently hid it.

use serde_json::Value;

use crate::model::{now_ms, UsageLimit, UsageSnapshot};

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
    // A scoped cap (one model, one product) at 100% doesn't block the account —
    // other models still work — so it colours its own gauge instead of `status`.
    let status = Some(if limited { "rejected" } else { "allowed" }.to_string());

    Some(UsageSnapshot {
        five_hour_utilization,
        seven_day_utilization,
        status,
        resets_at,
        updated_at: now_ms(),
        scoped_limits: scoped_limits(body),
    })
}

/// Every entry of `limits` except the session / all-models weekly pair, which
/// the dedicated 5-hour and 7-day gauges already show.
fn scoped_limits(body: &Value) -> Vec<UsageLimit> {
    let Some(limits) = body.get("limits").and_then(Value::as_array) else {
        return Vec::new();
    };
    limits
        .iter()
        .filter_map(|l| {
            let kind = l.get("kind")?.as_str()?;
            if kind == "session" || kind == "weekly_all" {
                return None;
            }
            Some(UsageLimit {
                label: limit_label(kind, l.get("group").and_then(Value::as_str), l.get("scope")),
                percent: l.get("percent")?.as_f64()?,
                severity: l
                    .get("severity")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                resets_at: l
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .and_then(parse_epoch),
            })
        })
        .collect()
}

/// `"7-day · Fable"` for a weekly model cap; falls back to the raw kind so an
/// unfamiliar limit still shows up rather than vanishing.
fn limit_label(kind: &str, group: Option<&str>, scope: Option<&Value>) -> String {
    let window = match group {
        Some("weekly") => "7-day",
        Some("session") => "5-hour",
        _ => "",
    };
    let scope_name = scope.and_then(|s| {
        s.pointer("/model/display_name")
            .and_then(Value::as_str)
            .or_else(|| s.pointer("/surface/display_name").and_then(Value::as_str))
            .or_else(|| s.get("surface").and_then(Value::as_str))
    });
    match (window, scope_name) {
        ("", Some(name)) => name.to_string(),
        (w, Some(name)) => format!("{w} · {name}"),
        ("", None) => kind.replace('_', " "),
        (w, None) => format!("{w} · {}", kind.replace('_', " ")),
    }
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
    fn surfaces_the_model_scoped_weekly_cap() {
        // Trimmed from a live 2026-09-19 response: the Fable cap lives only in `limits`.
        let body = json!({
            "five_hour": { "utilization": 9.0, "resets_at": "2026-09-19T18:20:00+00:00" },
            "seven_day": { "utilization": 77.0, "resets_at": "2026-09-20T06:59:59+00:00" },
            "seven_day_opus": null,
            "limits": [
                { "kind": "session", "group": "session", "percent": 9, "severity": "normal",
                  "resets_at": "2026-09-19T18:20:00+00:00", "scope": null },
                { "kind": "weekly_all", "group": "weekly", "percent": 77, "severity": "warning",
                  "resets_at": "2026-09-20T06:59:59+00:00", "scope": null },
                { "kind": "weekly_scoped", "group": "weekly", "percent": 94, "severity": "critical",
                  "resets_at": "2026-09-20T07:00:00+00:00",
                  "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null } }
            ]
        });
        let snap = parse(&body).unwrap();
        assert_eq!(snap.scoped_limits.len(), 1);
        let fable = &snap.scoped_limits[0];
        assert_eq!(fable.label, "7-day · Fable");
        assert_eq!(fable.percent, 94.0);
        assert_eq!(fable.severity.as_deref(), Some("critical"));
        assert!(fable.resets_at.is_some());
        // A single model's cap doesn't mark the whole account limited.
        assert_eq!(snap.status.as_deref(), Some("allowed"));
    }

    #[test]
    fn unfamiliar_limits_still_show_up() {
        let body = json!({
            "five_hour": { "utilization": 1.0, "resets_at": null },
            "limits": [ { "kind": "monthly_thing", "group": null, "percent": 5, "scope": null } ]
        });
        let snap = parse(&body).unwrap();
        assert_eq!(snap.scoped_limits[0].label, "monthly thing");
    }

    #[test]
    fn none_when_no_known_window() {
        let body = json!({ "error": "nope" });
        assert!(parse(&body).is_none());
    }
}
