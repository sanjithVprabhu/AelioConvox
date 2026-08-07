//! Bridge reuse counters (Phase 2 Session C, §14.1).
//!
//! Raw counters only — no ratios at the emitter. A daily log line carries the distribution
//! verdict via `executions_per_key`.

use crate::abilities::matching::{Gate, MatchDecision, Outcome};
use crate::types::LookupTier;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Default)]
struct ReuseMetricsInner {
    cold_executions: u64,
    warm_hits: u64,
    distinct_situation_keys: BTreeMap<String, u64>,
    residue_rejections_total: u64,
    gate_decisions_total: BTreeMap<(Gate, Outcome), u64>,
}

static METRICS: OnceLock<Mutex<ReuseMetricsInner>> = OnceLock::new();

fn metrics() -> &'static Mutex<ReuseMetricsInner> {
    METRICS.get_or_init(|| Mutex::new(ReuseMetricsInner::default()))
}

/// Record a lookup tier decision against the situation hash for this turn.
pub fn record_lookup(tier: LookupTier, situation_hash: &str) {
    let mut guard = metrics().lock().expect("reuse metrics lock");
    match tier {
        LookupTier::Tier0 | LookupTier::Tier1 => guard.warm_hits += 1,
        LookupTier::Tier2 | LookupTier::Tier3 => guard.cold_executions += 1,
    }
    *guard
        .distinct_situation_keys
        .entry(situation_hash.to_string())
        .or_insert(0) += 1;
}

/// Record the named gate that admitted or rejected a warm candidate. Gate failures are counted
/// separately from lookup misses so a dangerously quiet residue gate is observable.
pub fn record_match_decision(decision: &MatchDecision) {
    let mut guard = metrics().lock().expect("reuse metrics lock");
    *guard
        .gate_decisions_total
        .entry((decision.gate, decision.outcome))
        .or_insert(0) += 1;
    if decision.gate == Gate::Residue && decision.outcome != Outcome::Accepted {
        guard.residue_rejections_total += 1;
    }
}

/// Snapshot of raw counters (for tests and daily logging).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReuseMetricsSnapshot {
    pub cold_executions: u64,
    pub warm_hits: u64,
    pub distinct_situation_keys: usize,
    pub executions_per_key: BTreeMap<String, u64>,
    pub residue_rejections_total: u64,
    pub gate_decisions_total: BTreeMap<(Gate, Outcome), u64>,
}

pub fn snapshot() -> ReuseMetricsSnapshot {
    let guard = metrics().lock().expect("reuse metrics lock");
    ReuseMetricsSnapshot {
        cold_executions: guard.cold_executions,
        warm_hits: guard.warm_hits,
        distinct_situation_keys: guard.distinct_situation_keys.len(),
        executions_per_key: guard.distinct_situation_keys.clone(),
        residue_rejections_total: guard.residue_rejections_total,
        gate_decisions_total: guard.gate_decisions_total.clone(),
    }
}

/// Daily log line — raw counters only, no precomputed ratios (§14.1).
pub fn daily_log_line() -> String {
    let snap = snapshot();
    format!(
        "reuse_metrics cold_executions={} warm_hits={} residue_rejections_total={} gate_decisions_total={:?} distinct_situation_keys={} executions_per_key={:?}",
        snap.cold_executions,
        snap.warm_hits,
        snap.residue_rejections_total,
        snap.gate_decisions_total,
        snap.distinct_situation_keys,
        snap.executions_per_key
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier0_is_warm_tier3_is_cold() {
        let before = snapshot();
        record_lookup(LookupTier::Tier3, "sigma-a");
        record_lookup(LookupTier::Tier0, "sigma-a");
        record_lookup(LookupTier::Tier1, "sigma-b");
        let snap = snapshot();
        assert_eq!(snap.cold_executions, before.cold_executions + 1);
        assert_eq!(snap.warm_hits, before.warm_hits + 2);
        assert_eq!(
            snap.executions_per_key.get("sigma-a"),
            Some(
                &(before
                    .executions_per_key
                    .get("sigma-a")
                    .copied()
                    .unwrap_or(0)
                    + 2)
            )
        );
        assert!(daily_log_line().contains(&format!("warm_hits={}", before.warm_hits + 2)));
    }
}
