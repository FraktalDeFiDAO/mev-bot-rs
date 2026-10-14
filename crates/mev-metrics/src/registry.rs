//! Scrape-time registry. Nothing here is on a hot path: producers either call
//! [`Registry::observe_event`] with an event they were emitting anyway (shadow scheduler passes, a few
//! per minute, already off the hot path) or register a collector that renders from a snapshot. A scrape
//! locks the small state struct only long enough to write a few dozen lines.
//!
//! Series are only emitted for things something measured: a family appears after the first event that
//! carries it. Nothing is faked with a placeholder zero.

use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use crate::prom::{Exposition, Kind};

/// Families the scheduler/advisory event ingestion can produce (all of them once both events were seen).
pub const SCHED_FAMILIES: &[&str] = &[
    "copper_liq_recheck_queue_depth",
    "copper_liq_recheck_deadline_miss_total",
    "copper_liq_coverage_violation_total",
    "copper_liq_sched_passes_total",
    "copper_liq_sched_last_pass_timestamp_seconds",
    "copper_jev_fallback_total",
    "copper_jev_stale_response_total",
    "copper_jev_applied_bonus_total",
    "copper_liq_advisory_passes_total",
    "copper_liq_advisory_mandatory_changed_total",
];

pub const EVENT_SCHEDULE_COUNTERS: &str = "liq_sched_counters";
pub const EVENT_ADVISORY: &str = "liq_advisory_shadow";
/// Periodic census of the snapshot repository (engine `liq_shadow`).
pub const EVENT_CENSUS: &str = "liq_shadow_census";
/// One refresh round of the snapshot repository (engine `liq_shadow`).
pub const EVENT_REFRESH: &str = "liq_shadow_refresh";

/// Families the liquidation pipeline events produce (census + refresh).
pub const PIPELINE_FAMILIES: &[&str] = &[
    "copper_liq_accounts",
    "copper_liq_census_timestamp_seconds",
    "copper_liq_rechecks_total",
];
/// `state` label values of `copper_liq_accounts`, in census field order.
pub const CENSUS_STATES: [&str; 9] = [
    "active",
    "inactive",
    "unknown",
    "hot",
    "hot_fresh",
    "cold",
    "cold_fresh",
    "dirty",
    "eligible",
];
const RECHECK_RESULTS: [&str; 3] = ["stored", "superseded", "rpc_error"];

const TIERS: [&str; 4] = ["p0", "p1", "p2", "p3"];
/// `tier` label of the deadline-miss counter: the shadow counters aggregate P2 and P3.
const MISS_TIERS: [&str; 3] = ["p0", "p1", "discretionary"];
const MISS_REASONS: [&str; 2] = ["backlog", "late_dispatch"];
/// Distinct label values accepted per advisory map before the rest folds into `other`.
const MAX_KEYS: usize = 32;

type Collector = Box<dyn Fn(&mut Exposition) + Send + Sync>;

#[derive(Default)]
struct SchedState {
    seen: bool,
    last_now_ns: u64,
    queue_depth: [u64; 4],
    /// `[tier][reason]`.
    misses: [[u64; 2]; 3],
    coverage_violations: u64,
    passes: u64,
    last_pass_ts: u64,
}

#[derive(Default)]
struct AdvisoryState {
    seen: bool,
    passes: u64,
    fallback: BTreeMap<String, u64>,
    stale: u64,
    bonus: BTreeMap<String, u64>,
    mandatory_changed: u64,
}

#[derive(Default)]
struct PipelineState {
    census_seen: bool,
    census: [u64; 9],
    census_ts: u64,
    refresh_seen: bool,
    /// Indexed like `RECHECK_RESULTS`.
    rechecks: [u64; 3],
}

#[derive(Default)]
struct State {
    sched: SchedState,
    advisory: AdvisoryState,
    pipeline: PipelineState,
}

#[derive(Default)]
pub struct Registry {
    state: Mutex<State>,
    collectors: Mutex<Vec<Collector>>,
}

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

fn add_map(dst: &mut BTreeMap<String, u64>, src: &Value) {
    let Some(m) = src.as_object() else { return };
    for (k, v) in m {
        let Some(n) = v.as_u64() else { continue };
        let k = crate::prom::bound_label_value(k);
        let key = if dst.contains_key(k) || dst.len() < MAX_KEYS {
            k
        } else {
            "other"
        };
        let e = dst.entry(key.to_string()).or_default();
        *e = e.saturating_add(n);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a collector rendered on every scrape (it must only read a prepared snapshot).
    pub fn add_collector(&self, f: impl Fn(&mut Exposition) + Send + Sync + 'static) {
        if let Ok(mut c) = self.collectors.lock() {
            c.push(Box::new(f));
        }
    }

    /// Feed one metrics event. Unknown kinds and malformed payloads are ignored. Each scheduler pass is
    /// accounted once: a repeated or older `now_ns` is dropped, so re-delivery cannot double count.
    pub fn observe_event(&self, kind: &str, data: &Value) {
        match kind {
            EVENT_SCHEDULE_COUNTERS => self.observe_sched(data),
            EVENT_ADVISORY => self.observe_advisory(data),
            EVENT_CENSUS => self.observe_census(data),
            EVENT_REFRESH => self.observe_refresh(data),
            _ => {}
        }
    }

    fn observe_sched(&self, d: &Value) {
        let (Some(now_ns), Some(c)) = (u(d, "now_ns"), d.get("candidates")) else {
            return;
        };
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        let s = &mut st.sched;
        if s.seen && now_ns <= s.last_now_ns {
            return;
        }
        s.seen = true;
        s.last_now_ns = now_ns;
        for (i, t) in TIERS.iter().enumerate() {
            s.queue_depth[i] = u(c, t).unwrap_or(0);
        }
        // Per-pass observations: a job that stays overdue is counted again by the next pass.
        let g = |k: &str| u(d, k).unwrap_or(0);
        let rows = [
            (g("deadline_misses_p0"), g("deadline_misses_backlog_p0")),
            (g("deadline_misses_p1"), g("deadline_misses_backlog_p1")),
            (
                g("deadline_misses_other"),
                g("deadline_misses_backlog_other"),
            ),
        ];
        for (i, (total, backlog)) in rows.into_iter().enumerate() {
            let backlog = backlog.min(total);
            s.misses[i][0] = s.misses[i][0].saturating_add(backlog);
            s.misses[i][1] = s.misses[i][1].saturating_add(total - backlog);
        }
        s.coverage_violations = s
            .coverage_violations
            .saturating_add(g("coverage_violations"));
        s.passes = s.passes.saturating_add(1);
        s.last_pass_ts = now_secs();
    }

    fn observe_census(&self, d: &Value) {
        let Some(vals) = CENSUS_STATES
            .iter()
            .map(|k| u(d, k))
            .collect::<Option<Vec<u64>>>()
        else {
            return;
        };
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        let p = &mut st.pipeline;
        p.census_seen = true;
        p.census.copy_from_slice(&vals);
        p.census_ts = now_secs();
    }

    fn observe_refresh(&self, d: &Value) {
        let (Some(stored), Some(conflicts), Some(rpc)) =
            (u(d, "stored"), u(d, "conflicts"), u(d, "rpc_errors"))
        else {
            return;
        };
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        let p = &mut st.pipeline;
        p.refresh_seen = true;
        for (i, n) in [stored, conflicts, rpc].into_iter().enumerate() {
            p.rechecks[i] = p.rechecks[i].saturating_add(n);
        }
    }

    fn observe_advisory(&self, d: &Value) {
        let Some(unchanged) = d.get("mandatory_unchanged").and_then(Value::as_bool) else {
            return;
        };
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        let a = &mut st.advisory;
        a.seen = true;
        a.passes = a.passes.saturating_add(1);
        if !unchanged {
            a.mandatory_changed = a.mandatory_changed.saturating_add(1);
        }
        if let Some(c) = d.get("counters") {
            if let Some(m) = c.get("copper_jev_fallback_total") {
                add_map(&mut a.fallback, m);
            }
            if let Some(m) = c.get("copper_jev_applied_bonus_total") {
                add_map(&mut a.bonus, m);
            }
            a.stale = a
                .stale
                .saturating_add(u(c, "copper_jev_stale_response_total").unwrap_or(0));
        }
    }

    /// Render the whole exposition (text format 0.0.4).
    pub fn render(&self) -> String {
        let mut ex = Exposition::new();
        self.render_into(&mut ex);
        if let Ok(cs) = self.collectors.lock() {
            for c in cs.iter() {
                c(&mut ex);
            }
        }
        ex.finish()
    }

    fn render_into(&self, ex: &mut Exposition) {
        let Ok(st) = self.state.lock() else { return };
        let s = &st.sched;
        if s.seen {
            ex.family(
                "copper_liq_recheck_queue_depth",
                "Candidate recheck jobs per tier in the last shadow scheduler pass.",
                Kind::Gauge,
            );
            for (i, t) in TIERS.iter().enumerate() {
                ex.sample_u64(&[("tier", t)], s.queue_depth[i]);
            }
            ex.family(
                "copper_liq_recheck_deadline_miss_total",
                "Overdue jobs observed by shadow scheduler passes (a job that stays overdue counts once per pass).",
                Kind::Counter,
            );
            for (i, t) in MISS_TIERS.iter().enumerate() {
                for (j, r) in MISS_REASONS.iter().enumerate() {
                    ex.sample_u64(&[("tier", t), ("reason", r)], s.misses[i][j]);
                }
            }
            ex.family(
                "copper_liq_coverage_violation_total",
                "Coverage violations reported by shadow scheduler passes.",
                Kind::Counter,
            );
            ex.sample_u64(&[], s.coverage_violations);
            ex.family(
                "copper_liq_sched_passes_total",
                "Shadow scheduler passes observed.",
                Kind::Counter,
            );
            ex.sample_u64(&[], s.passes);
            ex.family(
                "copper_liq_sched_last_pass_timestamp_seconds",
                "Unix time at which the last shadow scheduler pass was observed.",
                Kind::Gauge,
            );
            ex.sample_u64(&[], s.last_pass_ts);
        }
        let p = &st.pipeline;
        if p.census_seen {
            ex.family(
                "copper_liq_accounts",
                "Active borrowers by snapshot state at the last census (hot = debt at or above the cutoff).",
                Kind::Gauge,
            );
            for (i, k) in CENSUS_STATES.iter().enumerate() {
                ex.sample_u64(&[("state", k)], p.census[i]);
            }
            ex.family(
                "copper_liq_census_timestamp_seconds",
                "Unix time of the last liquidation snapshot census.",
                Kind::Gauge,
            );
            ex.sample_u64(&[], p.census_ts);
        }
        if p.refresh_seen {
            ex.family(
                "copper_liq_rechecks_total",
                "Account rechecks by the snapshot refresh task, by result.",
                Kind::Counter,
            );
            for (i, r) in RECHECK_RESULTS.iter().enumerate() {
                ex.sample_u64(&[("result", r)], p.rechecks[i]);
            }
        }
        let a = &st.advisory;
        if a.seen {
            ex.family(
                "copper_jev_fallback_total",
                "Advisory policy verdicts that fell back to the baseline, by reason (cumulative over shadow passes).",
                Kind::Counter,
            );
            for (k, v) in &a.fallback {
                ex.sample_u64(&[("reason", k)], *v);
            }
            ex.family(
                "copper_jev_stale_response_total",
                "Advisory decisions rejected as stale or expired by the policy (cumulative over shadow passes).",
                Kind::Counter,
            );
            ex.sample_u64(&[], a.stale);
            ex.family(
                "copper_jev_applied_bonus_total",
                "Would-be tie-break bonuses granted by the advisory policy, by tier (shadow, cumulative).",
                Kind::Counter,
            );
            for (k, v) in &a.bonus {
                ex.sample_u64(&[("tier", k)], *v);
            }
            ex.family(
                "copper_liq_advisory_passes_total",
                "Advisory shadow passes observed.",
                Kind::Counter,
            );
            ex.sample_u64(&[], a.passes);
            ex.family(
                "copper_liq_advisory_mandatory_changed_total",
                "Passes in which the advisory ordering changed P0/P1 output (must stay 0).",
                Kind::Counter,
            );
            ex.sample_u64(&[], a.mandatory_changed);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sched(now: u64, p0_miss: u64, p0_backlog: u64) -> Value {
        json!({
            "now_ns": now,
            "candidates": {"p0": 1, "p1": 2, "p2": 3, "p3": 4},
            "deadline_misses_p0": p0_miss, "deadline_misses_p1": 0, "deadline_misses_other": 0,
            "deadline_misses_backlog_p0": p0_backlog, "deadline_misses_backlog_p1": 0,
            "deadline_misses_backlog_other": 0,
            "coverage_violations": 1
        })
    }

    #[test]
    fn census_and_refresh_events_become_gauges_and_counters() {
        let r = Registry::new();
        assert!(
            !r.render().contains("copper_liq_accounts"),
            "nothing before a census"
        );
        r.observe_event(
            EVENT_CENSUS,
            &json!({"active": 10, "inactive": 5, "unknown": 1, "hot": 4, "hot_fresh": 3,
                    "cold": 5, "cold_fresh": 5, "dirty": 2, "eligible": 1}),
        );
        r.observe_event(EVENT_CENSUS, &json!({"active": 1})); // malformed: ignored
        for _ in 0..2 {
            r.observe_event(
                EVENT_REFRESH,
                &json!({"candidates": 9, "attempted": 9, "stored": 7, "conflicts": 1, "rpc_errors": 1}),
            );
        }
        let t = r.render();
        assert!(t.contains("copper_liq_accounts{state=\"active\"} 10\n"));
        assert!(t.contains("copper_liq_accounts{state=\"hot_fresh\"} 3\n"));
        assert!(t.contains("copper_liq_accounts{state=\"eligible\"} 1\n"));
        assert!(t.contains("copper_liq_rechecks_total{result=\"stored\"} 14\n"));
        assert!(t.contains("copper_liq_rechecks_total{result=\"rpc_error\"} 2\n"));
        assert!(t.contains("copper_liq_census_timestamp_seconds "));
    }

    #[test]
    fn nothing_is_emitted_before_anything_was_measured() {
        assert_eq!(Registry::new().render(), "");
    }

    #[test]
    fn passes_accumulate_once_and_replays_are_ignored() {
        let r = Registry::new();
        r.observe_event(EVENT_SCHEDULE_COUNTERS, &sched(10, 3, 1));
        r.observe_event(EVENT_SCHEDULE_COUNTERS, &sched(10, 3, 1));
        r.observe_event(EVENT_SCHEDULE_COUNTERS, &sched(5, 3, 1));
        r.observe_event(EVENT_SCHEDULE_COUNTERS, &sched(20, 2, 2));
        let t = r.render();
        assert!(t.contains(
            "copper_liq_recheck_deadline_miss_total{tier=\"p0\",reason=\"backlog\"} 3\n"
        ));
        assert!(t.contains(
            "copper_liq_recheck_deadline_miss_total{tier=\"p0\",reason=\"late_dispatch\"} 2\n"
        ));
        assert!(t.contains("copper_liq_sched_passes_total 2\n"));
        assert!(t.contains("copper_liq_coverage_violation_total 2\n"));
        assert!(t.contains("copper_liq_recheck_queue_depth{tier=\"p3\"} 4\n"));
    }

    #[test]
    fn advisory_counters_accumulate_across_passes_and_flag_a_mandatory_change() {
        let r = Registry::new();
        let ev = |unchanged: bool| {
            json!({
                "mandatory_unchanged": unchanged,
                "counters": {
                    "copper_jev_fallback_total": {"expired": 2, "no_decision": 1},
                    "copper_jev_stale_response_total": 1,
                    "copper_jev_applied_bonus_total": {"high": 4}
                }
            })
        };
        r.observe_event(EVENT_ADVISORY, &ev(true));
        r.observe_event(EVENT_ADVISORY, &ev(false));
        let t = r.render();
        assert!(t.contains("copper_jev_fallback_total{reason=\"expired\"} 4\n"));
        assert!(t.contains("copper_jev_stale_response_total 2\n"));
        assert!(t.contains("copper_jev_applied_bonus_total{tier=\"high\"} 8\n"));
        assert!(t.contains("copper_liq_advisory_mandatory_changed_total 1\n"));
        assert!(t.contains("copper_liq_advisory_passes_total 2\n"));
    }

    #[test]
    fn malformed_events_and_address_keys_are_harmless() {
        let r = Registry::new();
        r.observe_event(EVENT_SCHEDULE_COUNTERS, &json!({"now_ns": "x"}));
        r.observe_event(EVENT_ADVISORY, &json!({"counters": {}}));
        r.observe_event("other_kind", &json!({}));
        assert_eq!(r.render(), "");
        let addr = ["0x", &"cd".repeat(20)].concat();
        r.observe_event(
            EVENT_ADVISORY,
            &json!({"mandatory_unchanged": true, "counters": {"copper_jev_fallback_total": {addr.clone(): 1}}}),
        );
        let t = r.render();
        assert!(!t.contains(&addr));
        assert!(t.contains("reason=\"other\""));
    }

    #[test]
    fn distinct_label_values_are_capped() {
        let r = Registry::new();
        let mut m = serde_json::Map::new();
        for i in 0..200 {
            m.insert(format!("r{i}"), json!(1));
        }
        r.observe_event(
            EVENT_ADVISORY,
            &json!({"mandatory_unchanged": true, "counters": {"copper_jev_fallback_total": m}}),
        );
        let series = r
            .render()
            .lines()
            .filter(|l| l.starts_with("copper_jev_fallback_total{"))
            .count();
        assert!(series <= MAX_KEYS + 1, "{series}");
    }
}
