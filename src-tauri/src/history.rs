//! Usage history and burn-rate forecasts.
//!
//! Every poll (≈ every 2 minutes) appends one line per account to
//! `history.jsonl` in the app data directory: the time and each limit's
//! percentage. The last [`RETENTION_DAYS`] are kept in memory too, which is small
//! (a few thousand points per account) and makes charts and forecasts cheap.
//!
//! A forecast is a least-squares slope over the recent points of one limit,
//! restarted after any reset (a drop in usage), extrapolated to 100%.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::UsageSnapshot;

pub const RETENTION_DAYS: i64 = 30;
const DAY_MS: i64 = 86_400_000;

/// How far back a forecast looks. Long enough to smooth one burst, short enough
/// to follow a change of pace.
const FORECAST_WINDOW_MS: i64 = 3 * 3_600_000;
/// Fewer points than this and the slope is noise.
const FORECAST_MIN_POINTS: usize = 4;

/// One account's limits at one moment. Keys are limit labels: `"Session"`,
/// `"Week"`, and scoped ones such as `"7-day · Fable"`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Point {
    pub t: i64,
    pub account: String,
    pub limits: BTreeMap<String, f64>,
}

impl Point {
    pub fn from_snapshot(account: &str, snap: &UsageSnapshot) -> Self {
        let mut limits = BTreeMap::new();
        if let Some(v) = snap.five_hour_utilization {
            limits.insert("Session".to_string(), v);
        }
        if let Some(v) = snap.seven_day_utilization {
            limits.insert("Week".to_string(), v);
        }
        for l in &snap.scoped_limits {
            limits.insert(l.label.clone(), l.percent);
        }
        Point {
            t: snap.updated_at,
            account: account.to_string(),
            limits,
        }
    }
}

/// When a limit will run out at the current pace.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Forecast {
    pub label: String,
    /// Percentage points per hour over the forecast window.
    pub per_hour: f64,
    /// Minutes until 100%, when usage is rising.
    pub minutes_to_full: Option<i64>,
}

#[derive(Default)]
pub struct History {
    file: Option<PathBuf>,
    points: Vec<Point>,
}

impl History {
    /// Load and prune the history file in `data_dir`.
    pub fn open(data_dir: &Path, now: i64) -> Self {
        let file = data_dir.join("history.jsonl");
        let cutoff = now - RETENTION_DAYS * DAY_MS;
        let points: Vec<Point> = std::fs::read_to_string(&file)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Point>(l).ok())
            .filter(|p| p.t >= cutoff)
            .collect();
        // Rewrite pruned so the file can't grow without bound.
        let body: String = points
            .iter()
            .filter_map(|p| serde_json::to_string(p).ok())
            .map(|l| l + "\n")
            .collect();
        let _ = std::fs::create_dir_all(data_dir);
        let _ = std::fs::write(&file, body);
        History {
            file: Some(file),
            points,
        }
    }

    pub fn push(&mut self, point: Point) {
        if let Some(file) = &self.file {
            if let Ok(line) = serde_json::to_string(&point) {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(file)
                {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
        self.points.push(point);
    }

    /// An account's points since `since` (ms), oldest first.
    pub fn series(&self, account: &str, since: i64) -> Vec<Point> {
        self.points
            .iter()
            .filter(|p| p.account == account && p.t >= since)
            .cloned()
            .collect()
    }

    /// Forecasts for every limit the account's latest point reports.
    pub fn forecasts(&self, account: &str, now: i64) -> Vec<Forecast> {
        let recent = self.series(account, now - FORECAST_WINDOW_MS);
        let Some(last) = recent.last() else {
            return Vec::new();
        };
        last.limits
            .keys()
            .filter_map(|label| forecast(&recent, label))
            .collect()
    }
}

/// Least-squares forecast for one limit over `points` (oldest first), using only
/// the run since the most recent reset.
pub fn forecast(points: &[Point], label: &str) -> Option<Forecast> {
    let series: Vec<(i64, f64)> = points
        .iter()
        .filter_map(|p| p.limits.get(label).map(|v| (p.t, *v)))
        .collect();
    // Start after the last drop: a reset makes everything before it irrelevant.
    let start = series
        .windows(2)
        .rposition(|w| w[1].1 + 1.0 < w[0].1)
        .map(|i| i + 1)
        .unwrap_or(0);
    let run = &series[start..];
    if run.len() < FORECAST_MIN_POINTS {
        return None;
    }

    let t0 = run[0].0 as f64;
    let n = run.len() as f64;
    let xs: Vec<f64> = run
        .iter()
        .map(|(t, _)| (*t as f64 - t0) / 3_600_000.0)
        .collect();
    let ys: Vec<f64> = run.iter().map(|(_, v)| *v).collect();
    let mx = xs.iter().sum::<f64>() / n;
    let my = ys.iter().sum::<f64>() / n;
    let sxx: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    if sxx < 1e-9 {
        return None;
    }
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let per_hour = sxy / sxx;

    let current = *ys.last()?;
    let minutes_to_full = (per_hour > 0.05 && current < 100.0)
        .then(|| ((100.0 - current) / per_hour * 60.0).round() as i64);

    Some(Forecast {
        label: label.to_string(),
        per_hour: (per_hour * 10.0).round() / 10.0,
        minutes_to_full,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(min: i64, v: f64) -> Point {
        Point {
            t: min * 60_000,
            account: "a".into(),
            limits: BTreeMap::from([("Week".to_string(), v)]),
        }
    }

    #[test]
    fn steady_climb_extrapolates_to_full() {
        // +10 points per hour, now at 70 → 3 hours left.
        let pts: Vec<Point> = (0..=6)
            .map(|i| pt(i * 10, 60.0 + i as f64 * 10.0 / 6.0))
            .collect();
        let f = forecast(&pts, "Week").unwrap();
        assert!((f.per_hour - 10.0).abs() < 0.2, "{f:?}");
        assert_eq!(f.minutes_to_full, Some(180));
    }

    #[test]
    fn flat_usage_has_no_eta() {
        let pts: Vec<Point> = (0..6).map(|i| pt(i * 10, 40.0)).collect();
        assert_eq!(forecast(&pts, "Week").unwrap().minutes_to_full, None);
    }

    #[test]
    fn restarts_after_a_reset() {
        let mut pts: Vec<Point> = (0..5).map(|i| pt(i * 10, 90.0 + i as f64)).collect();
        pts.push(pt(60, 2.0)); // reset
        assert!(
            forecast(&pts, "Week").is_none(),
            "too few points since the reset"
        );
    }

    #[test]
    fn point_captures_every_limit() {
        let snap = UsageSnapshot {
            five_hour_utilization: Some(9.0),
            seven_day_utilization: Some(77.0),
            scoped_limits: vec![crate::model::UsageLimit {
                label: "7-day · Fable".into(),
                percent: 94.0,
                severity: None,
                resets_at: None,
            }],
            updated_at: 5,
            ..UsageSnapshot::default()
        };
        let p = Point::from_snapshot("a", &snap);
        assert_eq!(p.limits.len(), 3);
        assert_eq!(p.limits["7-day · Fable"], 94.0);
    }
}
