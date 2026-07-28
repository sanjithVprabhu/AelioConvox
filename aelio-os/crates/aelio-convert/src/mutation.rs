//! Mutation harness (§16.6, committed P1 deliverable) — generate deliberately-wrong converters and
//! measure the gate's catch rate per phase. "The gate caught N% of mutated converters; here's the
//! harness, run it yourself" is both the credibility argument and a regression suite.

use crate::gate::{self, Digest, Evidence, Thresholds};
use crate::rules::Rule;
use aelio_sol::SolValue;

/// Per-phase catch accounting over a mutant population.
#[derive(Debug, Clone, Default)]
pub struct MutationReport {
    pub total: usize,
    pub caught_structural: usize,
    pub caught_shadow: usize,
    pub escaped: usize,
}

impl MutationReport {
    pub fn catch_rate(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        (self.total - self.escaped) as f64 / self.total as f64
    }
}

/// Run each mutant through Structural then Shadow (validate-without-consume over the recorded
/// inputs), against the consumer `digest`. A mutant is caught if Structural rejects it, or if its
/// shadow validation rate over `inputs` falls below the shadow threshold (§16.4).
pub fn run_harness(
    inputs: &[SolValue],
    digest: &Digest,
    mutants: &[Vec<Rule>],
    th: &Thresholds,
) -> MutationReport {
    let mut report = MutationReport {
        total: mutants.len(),
        ..Default::default()
    };
    for rules in mutants {
        if gate::structural_ok(rules).is_err() {
            report.caught_structural += 1;
            continue;
        }
        let mut distinct = std::collections::BTreeMap::new();
        for input in inputs {
            let agreed = gate::shadow_validate(rules, input, digest);
            let entry = distinct.entry(aelio_sol::value_hash(input)).or_insert(true);
            *entry = *entry && agreed;
        }
        let passes = distinct.values().filter(|agreed| **agreed).count();
        let rate = if distinct.is_empty() {
            1.0
        } else {
            passes as f64 / distinct.len() as f64
        };
        let ev = Evidence {
            shadow_distinct: distinct.len() as u64,
            shadow_validation_rate: rate,
            ..Default::default()
        };
        if gate::shadow_to_canary(&ev, th) {
            // The mutant would have advanced — it escaped the shadow oracle.
            report.escaped += 1;
        } else {
            report.caught_shadow += 1;
        }
    }
    report
}
