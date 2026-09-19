//! Decide which notifications a new usage reading deserves.
//!
//! Pure: compares the previous and new snapshot for one account and returns
//! the messages to show. Each threshold fires once on the way up (a crossing,
//! not a level), so a limit sitting at 96% doesn't notify every two minutes;
//! and a limit that was well used and dropped back is reported as a reset.

use crate::model::UsageSnapshot;
use crate::settings::Settings;

/// Below this, a limit going back to ~0 isn't worth announcing.
const RESET_WORTH_NOTING: f64 = 50.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
}

/// Each limit as `(label, percent, resets_at)`.
fn limits(s: &UsageSnapshot) -> Vec<(String, f64, Option<i64>)> {
    let mut v = Vec::new();
    if let Some(p) = s.five_hour_utilization {
        v.push(("Session".to_string(), p, None));
    }
    if let Some(p) = s.seven_day_utilization {
        v.push(("Weekly".to_string(), p, None));
    }
    for l in &s.scoped_limits {
        v.push((pretty_scoped(&l.label), l.percent, l.resets_at));
    }
    v
}

/// "7-day · Fable" → "Fable weekly".
fn pretty_scoped(label: &str) -> String {
    match label.split_once(" · ") {
        Some(("7-day", name)) => format!("{name} weekly"),
        Some(("5-hour", name)) => format!("{name} session"),
        Some((w, name)) => format!("{name} {w}"),
        None => label.to_string(),
    }
}

pub fn evaluate(
    account: &str,
    prev: Option<&UsageSnapshot>,
    new: &UsageSnapshot,
    settings: &Settings,
    now_secs: i64,
) -> Vec<Alert> {
    // Without a previous reading there's no crossing to detect — this is also
    // what keeps a fresh launch from announcing every limit already above 80%.
    let (true, Some(prev)) = (settings.alerts_enabled, prev) else {
        return Vec::new();
    };
    let before = limits(prev);
    let mut out = Vec::new();

    for (label, now_pct, resets_at) in limits(new) {
        let Some((_, was, _)) = before.iter().find(|(l, _, _)| *l == label) else {
            continue;
        };
        // Highest threshold crossed by this reading, so a jump from 70 to 97
        // sends one alert (95), not two.
        if let Some(t) = settings
            .alert_thresholds
            .iter()
            .rev()
            .find(|t| *was < **t as f64 && now_pct >= **t as f64)
        {
            let when = resets_at
                .map(|r| format!(" — resets in {}", human_duration(r - now_secs)))
                .unwrap_or_default();
            out.push(Alert {
                title: if now_pct >= 100.0 {
                    format!("{label} limit reached")
                } else {
                    format!("{label} limit at {t}%")
                },
                body: format!("{account}{when}"),
            });
        }
        if settings.notify_on_reset && *was >= RESET_WORTH_NOTING && now_pct + 25.0 < *was {
            out.push(Alert {
                title: format!("{label} limit reset"),
                body: format!("{account} is back to {}%", now_pct.round()),
            });
        }
    }
    out
}

fn human_duration(secs: i64) -> String {
    let mins = (secs.max(0) + 59) / 60;
    match mins {
        m if m < 60 => format!("{m}m"),
        m if m < 24 * 60 => format!("{}h {}m", m / 60, m % 60),
        m => format!("{}d {}h", m / 1440, (m % 1440) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UsageLimit;

    fn snap(week: f64, fable: f64) -> UsageSnapshot {
        UsageSnapshot {
            five_hour_utilization: Some(10.0),
            seven_day_utilization: Some(week),
            scoped_limits: vec![UsageLimit {
                label: "7-day · Fable".into(),
                percent: fable,
                severity: None,
                resets_at: Some(3_600 * 14),
            }],
            ..UsageSnapshot::default()
        }
    }

    #[test]
    fn fires_once_per_crossing() {
        let s = Settings::default();
        let a = evaluate("me", Some(&snap(70.0, 90.0)), &snap(81.0, 96.0), &s, 0);
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0].title, "Weekly limit at 80%");
        assert_eq!(a[1].title, "Fable weekly limit at 95%");
        assert_eq!(a[1].body, "me — resets in 14h 0m");
        // Staying above the line doesn't repeat it.
        assert!(evaluate("me", Some(&snap(81.0, 96.0)), &snap(84.0, 97.0), &s, 0).is_empty());
    }

    #[test]
    fn a_jump_past_two_thresholds_sends_the_higher_one() {
        let a = evaluate(
            "me",
            Some(&snap(70.0, 10.0)),
            &snap(97.0, 10.0),
            &Settings::default(),
            0,
        );
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].title, "Weekly limit at 95%");
    }

    #[test]
    fn announces_resets_and_stays_quiet_on_first_reading() {
        let s = Settings::default();
        let a = evaluate("me", Some(&snap(99.0, 20.0)), &snap(1.0, 20.0), &s, 0);
        assert_eq!(a[0].title, "Weekly limit reset");
        assert!(evaluate("me", None, &snap(99.0, 99.0), &s, 0).is_empty());
    }

    #[test]
    fn disabled_means_silent() {
        let s = Settings {
            alerts_enabled: false,
            ..Settings::default()
        };
        assert!(evaluate("me", Some(&snap(70.0, 90.0)), &snap(99.0, 99.0), &s, 0).is_empty());
    }
}
