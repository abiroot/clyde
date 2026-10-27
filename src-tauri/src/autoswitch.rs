//! Automatic account switching: decide, after each usage poll, whether Claude
//! Code should move to another account.
//!
//! The policy is "use the highest-priority account that still has room":
//! - An account *trips* when the limit its rule watches reaches the rule's
//!   threshold (or Anthropic already rejects it). Each account follows the
//!   default rule unless it has its own.
//! - When the active account trips, switch to the first account in priority
//!   order that hasn't tripped and isn't excluded.
//! - When the active account is fine but a higher-priority account has room
//!   again (its limit reset), switch back — unless the user picked the current
//!   account by hand, which holds until it trips.
//!
//! Switching *to* an account needs [`HEADROOM`] points of room under its
//! threshold, so a limit hovering at the line can't make Clyde flap between
//! two accounts. Pure: the engine feeds it a view of every account and acts on
//! the answer.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::UsageSnapshot;

/// Points under its threshold an account needs before Clyde switches to it.
pub const HEADROOM: f64 = 5.0;

/// A usage reading older than this is too stale to act on (ms). Polls run
/// every two minutes, so this allows a couple of failed reads.
pub const STALE_MS: i64 = 10 * 60 * 1000;

/// When an account counts as used up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    /// Which limit to watch: `"any"` (the tightest), `"session"` (5-hour),
    /// `"weekly"` (7-day), or a per-model cap by its label, e.g.
    /// `"7-day · Fable"`.
    pub limit: String,
    /// Switch away at or above this percentage; 100 = only when it's full.
    pub at: u8,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            limit: "any".into(),
            at: 95,
        }
    }
}

impl Rule {
    /// The watched limit's current percentage. `None` when the account has no
    /// such limit (a model cap it doesn't carry) — that rule never trips.
    pub fn reading(&self, u: &UsageSnapshot) -> Option<f64> {
        match self.limit.as_str() {
            "any" => crate::engine::tightest(u),
            "session" => u.five_hour_utilization,
            "weekly" => u.seven_day_utilization,
            label => u
                .scoped_limits
                .iter()
                .find(|l| l.label == label)
                .map(|l| l.percent),
        }
    }

    /// Human form, e.g. "Session ≥ 90%".
    pub fn describe(&self) -> String {
        let what = match self.limit.as_str() {
            "any" => "Any limit".to_string(),
            "session" => "Session".to_string(),
            "weekly" => "Weekly".to_string(),
            label => crate::alerts::pretty_scoped(label),
        };
        if self.at >= 100 {
            format!("{what} full")
        } else {
            format!("{what} ≥ {}%", self.at)
        }
    }

    fn normalized(mut self) -> Self {
        self.at = self.at.clamp(1, 100);
        if self.limit.trim().is_empty() {
            self.limit = "any".into();
        }
        self
    }
}

/// Per-account settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountPolicy {
    /// Never switch *to* this account automatically. Picking it by hand still
    /// works, and Clyde still moves off it when it trips.
    pub exclude: bool,
    /// Replaces the default rule for this account.
    pub rule: Option<Rule>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoSwitchSettings {
    pub enabled: bool,
    pub default_rule: Rule,
    /// Account ids, most preferred first. Accounts not listed rank after the
    /// listed ones, in the order Clyde stores them.
    pub priority: Vec<String>,
    /// Keyed by account id.
    pub accounts: HashMap<String, AccountPolicy>,
    /// Move back to a higher-priority account once it has room again.
    pub switch_back: bool,
    /// Notify on every automatic switch.
    pub notify: bool,
}

impl Default for AutoSwitchSettings {
    fn default() -> Self {
        AutoSwitchSettings {
            enabled: false,
            default_rule: Rule::default(),
            priority: Vec::new(),
            accounts: HashMap::new(),
            switch_back: true,
            notify: true,
        }
    }
}

impl AutoSwitchSettings {
    pub fn normalized(mut self) -> Self {
        self.default_rule = self.default_rule.normalized();
        let mut seen = std::collections::HashSet::new();
        self.priority.retain(|id| seen.insert(id.clone()));
        for p in self.accounts.values_mut() {
            p.rule = p.rule.take().map(Rule::normalized);
        }
        // Drop policies that say nothing.
        self.accounts.retain(|_, p| p.exclude || p.rule.is_some());
        self
    }

    pub fn rule_for(&self, id: &str) -> &Rule {
        self.accounts
            .get(id)
            .and_then(|p| p.rule.as_ref())
            .unwrap_or(&self.default_rule)
    }

    fn excluded(&self, id: &str) -> bool {
        self.accounts.get(id).is_some_and(|p| p.exclude)
    }

    /// `ids` (Clyde's storage order) sorted by priority.
    pub fn ordered<'a>(&self, ids: &[&'a str]) -> Vec<&'a str> {
        let mut out: Vec<&str> = self
            .priority
            .iter()
            .filter_map(|p| ids.iter().find(|id| **id == p.as_str()).copied())
            .collect();
        for id in ids {
            if !out.contains(id) {
                out.push(id);
            }
        }
        out
    }
}

/// What the engine knows about one account.
#[derive(Clone, Debug)]
pub struct Candidate<'a> {
    pub id: &'a str,
    /// The latest reading, if there is one and it read cleanly.
    pub usage: Option<&'a UsageSnapshot>,
    /// Its saved login still works.
    pub healthy: bool,
}

/// What [`decide`] concluded.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Nothing to do.
    Stay,
    /// Switch to `to`.
    Switch { to: String, reason: Reason },
    /// The active account tripped and no other account can take over.
    Exhausted { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    /// The active account hit its rule, e.g. "Session ≥ 90%".
    Tripped(String),
    /// A higher-priority account has room again.
    SwitchBack,
}

fn fresh(u: &UsageSnapshot, now_ms: i64) -> bool {
    u.updated_at > 0 && now_ms - u.updated_at <= STALE_MS
}

/// The reason `id` is used up, if it is.
fn tripped(cfg: &AutoSwitchSettings, id: &str, u: &UsageSnapshot) -> Option<String> {
    let rule = cfg.rule_for(id);
    if u.status.as_deref() == Some("rejected") {
        return Some("limit reached".into());
    }
    let p = rule.reading(u)?;
    (p >= rule.at as f64).then(|| rule.describe())
}

/// Has enough room to switch to.
fn has_room(cfg: &AutoSwitchSettings, c: &Candidate, now_ms: i64) -> bool {
    let Some(u) = c.usage else { return false };
    if !c.healthy
        || !fresh(u, now_ms)
        || cfg.excluded(c.id)
        || u.status.as_deref() == Some("rejected")
    {
        return false;
    }
    let rule = cfg.rule_for(c.id);
    match rule.reading(u) {
        Some(p) => p < rule.at as f64 - HEADROOM,
        // No such limit on this account: nothing to run out of.
        None => true,
    }
}

/// `manual_hold`: the user picked the active account by hand, so don't switch
/// back from it until it trips.
pub fn decide(
    cfg: &AutoSwitchSettings,
    active: Option<&str>,
    accounts: &[Candidate],
    manual_hold: bool,
    now_ms: i64,
) -> Decision {
    if !cfg.enabled {
        return Decision::Stay;
    }
    let Some(active) = active else {
        return Decision::Stay;
    };
    let Some(current) = accounts.iter().find(|c| c.id == active) else {
        return Decision::Stay;
    };

    let ids: Vec<&str> = accounts.iter().map(|c| c.id).collect();
    let order = cfg.ordered(&ids);
    let first_with_room = |among: &[&str]| -> Option<String> {
        among
            .iter()
            .find(|id| {
                **id != active
                    && accounts
                        .iter()
                        .find(|c| c.id == **id)
                        .is_some_and(|c| has_room(cfg, c, now_ms))
            })
            .map(|id| id.to_string())
    };

    let current_trip = current
        .usage
        .filter(|u| fresh(u, now_ms))
        .and_then(|u| tripped(cfg, active, u));

    if let Some(reason) = current_trip {
        return match first_with_room(&order) {
            Some(to) => Decision::Switch {
                to,
                reason: Reason::Tripped(reason),
            },
            None => Decision::Exhausted { reason },
        };
    }

    // Healthy and under its limit (or unreadable — never move on a guess).
    if !cfg.switch_back || manual_hold || current.usage.is_none_or(|u| !fresh(u, now_ms)) {
        return Decision::Stay;
    }
    let rank = order
        .iter()
        .position(|id| *id == active)
        .unwrap_or(order.len());
    match first_with_room(&order[..rank]) {
        Some(to) => Decision::Switch {
            to,
            reason: Reason::SwitchBack,
        },
        None => Decision::Stay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UsageLimit;

    const NOW: i64 = 1_000_000_000;

    fn u(session: f64, week: f64) -> UsageSnapshot {
        UsageSnapshot {
            five_hour_utilization: Some(session),
            seven_day_utilization: Some(week),
            updated_at: NOW - 60_000,
            ..UsageSnapshot::default()
        }
    }

    fn cfg(priority: &[&str]) -> AutoSwitchSettings {
        AutoSwitchSettings {
            enabled: true,
            priority: priority.iter().map(|s| s.to_string()).collect(),
            ..AutoSwitchSettings::default()
        }
    }

    fn c<'a>(id: &'a str, usage: &'a UsageSnapshot) -> Candidate<'a> {
        Candidate {
            id,
            usage: Some(usage),
            healthy: true,
        }
    }

    fn switch_to(d: &Decision) -> Option<&str> {
        match d {
            Decision::Switch { to, .. } => Some(to),
            _ => None,
        }
    }

    #[test]
    fn disabled_never_switches() {
        let (a, b) = (u(99.0, 10.0), u(1.0, 1.0));
        let mut s = cfg(&["a", "b"]);
        s.enabled = false;
        assert_eq!(
            decide(&s, Some("a"), &[c("a", &a), c("b", &b)], false, NOW),
            Decision::Stay
        );
    }

    #[test]
    fn trips_to_the_first_account_in_priority_with_room() {
        let (a, b, d) = (u(96.0, 10.0), u(97.0, 10.0), u(5.0, 5.0));
        let s = cfg(&["a", "b", "d"]);
        let got = decide(
            &s,
            Some("a"),
            &[c("a", &a), c("b", &b), c("d", &d)],
            false,
            NOW,
        );
        assert_eq!(switch_to(&got), Some("d"));
        assert_eq!(
            got,
            Decision::Switch {
                to: "d".into(),
                reason: Reason::Tripped("Any limit ≥ 95%".into())
            }
        );
    }

    #[test]
    fn excluded_and_broken_accounts_are_skipped() {
        let (a, b, d) = (u(96.0, 10.0), u(1.0, 1.0), u(2.0, 2.0));
        let mut s = cfg(&["a", "b", "d"]);
        s.accounts.insert(
            "b".into(),
            AccountPolicy {
                exclude: true,
                rule: None,
            },
        );
        let mut broken = c("d", &d);
        broken.healthy = false;
        let got = decide(&s, Some("a"), &[c("a", &a), c("b", &b), broken], false, NOW);
        assert!(matches!(got, Decision::Exhausted { .. }), "{got:?}");
    }

    #[test]
    fn per_account_rules_override_the_default() {
        // a only cares about its weekly limit at 80%.
        let (a, b) = (u(99.0, 50.0), u(1.0, 1.0));
        let mut s = cfg(&["a", "b"]);
        s.accounts.insert(
            "a".into(),
            AccountPolicy {
                exclude: false,
                rule: Some(Rule {
                    limit: "weekly".into(),
                    at: 80,
                }),
            },
        );
        assert_eq!(
            decide(&s, Some("a"), &[c("a", &a), c("b", &b)], false, NOW),
            Decision::Stay
        );
        let a2 = u(10.0, 81.0);
        let got = decide(&s, Some("a"), &[c("a", &a2), c("b", &b)], false, NOW);
        assert_eq!(
            got,
            Decision::Switch {
                to: "b".into(),
                reason: Reason::Tripped("Weekly ≥ 80%".into())
            }
        );
    }

    #[test]
    fn watches_a_model_cap_by_label() {
        let mut a = u(10.0, 10.0);
        a.scoped_limits.push(UsageLimit {
            label: "7-day · Fable".into(),
            percent: 100.0,
            severity: None,
            resets_at: None,
        });
        let b = u(1.0, 1.0);
        let mut s = cfg(&["a", "b"]);
        s.default_rule = Rule {
            limit: "7-day · Fable".into(),
            at: 100,
        };
        let got = decide(&s, Some("a"), &[c("a", &a), c("b", &b)], false, NOW);
        assert_eq!(switch_to(&got), Some("b"));
        // b has no Fable cap, so its rule can't trip — it counts as having room.
    }

    #[test]
    fn switches_back_when_the_preferred_account_resets() {
        let (a, b) = (u(20.0, 20.0), u(30.0, 30.0));
        let s = cfg(&["a", "b"]);
        let got = decide(&s, Some("b"), &[c("a", &a), c("b", &b)], false, NOW);
        assert_eq!(
            got,
            Decision::Switch {
                to: "a".into(),
                reason: Reason::SwitchBack
            }
        );
        // Not while the user holds b by hand, and not when switch-back is off.
        assert_eq!(
            decide(&s, Some("b"), &[c("a", &a), c("b", &b)], true, NOW),
            Decision::Stay
        );
        let mut off = s.clone();
        off.switch_back = false;
        assert_eq!(
            decide(&off, Some("b"), &[c("a", &a), c("b", &b)], false, NOW),
            Decision::Stay
        );
    }

    #[test]
    fn headroom_prevents_flapping_at_the_threshold() {
        // a sits just under its line: not enough room to go back to.
        let (a, b) = (u(92.0, 10.0), u(10.0, 10.0));
        let s = cfg(&["a", "b"]);
        assert_eq!(
            decide(&s, Some("b"), &[c("a", &a), c("b", &b)], false, NOW),
            Decision::Stay
        );
    }

    #[test]
    fn stale_or_missing_readings_never_trigger_a_switch() {
        let mut a = u(99.0, 99.0);
        a.updated_at = NOW - STALE_MS - 1;
        let b = u(1.0, 1.0);
        let s = cfg(&["a", "b"]);
        assert_eq!(
            decide(&s, Some("a"), &[c("a", &a), c("b", &b)], false, NOW),
            Decision::Stay
        );
        let unknown = Candidate {
            id: "a",
            usage: None,
            healthy: true,
        };
        assert_eq!(
            decide(&s, Some("a"), &[unknown, c("b", &b)], false, NOW),
            Decision::Stay
        );
    }

    #[test]
    fn rejected_counts_as_tripped_even_under_the_threshold() {
        let mut a = u(40.0, 40.0);
        a.status = Some("rejected".into());
        let b = u(1.0, 1.0);
        let got = decide(
            &cfg(&["a", "b"]),
            Some("a"),
            &[c("a", &a), c("b", &b)],
            false,
            NOW,
        );
        assert_eq!(
            got,
            Decision::Switch {
                to: "b".into(),
                reason: Reason::Tripped("limit reached".into())
            }
        );
    }

    #[test]
    fn unlisted_accounts_rank_last_in_storage_order() {
        let s = cfg(&["c"]);
        assert_eq!(s.ordered(&["a", "b", "c"]), vec!["c", "a", "b"]);
    }

    #[test]
    fn normalizes_rules_and_drops_empty_policies() {
        let mut s = AutoSwitchSettings {
            default_rule: Rule {
                limit: "".into(),
                at: 0,
            },
            priority: vec!["a".into(), "a".into()],
            ..AutoSwitchSettings::default()
        };
        s.accounts.insert("x".into(), AccountPolicy::default());
        let s = s.normalized();
        assert_eq!(
            s.default_rule,
            Rule {
                limit: "any".into(),
                at: 1
            }
        );
        assert_eq!(s.priority, vec!["a".to_string()]);
        assert!(s.accounts.is_empty());
    }
}
