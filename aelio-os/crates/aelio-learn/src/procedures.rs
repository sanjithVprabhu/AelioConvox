//! Procedure learning & promotion (§19). **A procedure is any subtree satisfying the Call-target
//! registry contract — promotion IS registration** (a promoted procedure is a registered Call
//! target, inheriting version pinning, DAG membership, policy gates, every §10 invariant, with zero
//! new machinery). The speculative bet, and the spec says so (§21 kill criterion).
//!
//! Mining trigger (v0, §19): **≥5 successful occurrences of an isomorphic op subsequence** (same
//! Call ids, compatible arg shapes) within a 30-day ledger window ⇒ propose. Bounded by construction
//! (contiguous subsequences up to a max length) so mining itself can't run unbounded.

use std::collections::BTreeMap;

/// One executed Call in a recorded flow: the target id + a structural signature of its args
/// (the `aelio_sol::structural_imprint` of the args, computed by the caller). "Isomorphic" = same
/// ids in order with compatible arg shapes, so the signature carries both.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TraceStep {
    pub call_id: String,
    pub arg_shape: String,
}

/// A recorded flow instance's Call sequence + outcome + the day it occurred (for the window).
#[derive(Debug, Clone)]
pub struct FlowTrace {
    pub steps: Vec<TraceStep>,
    pub success: bool,
    pub day: u32,
}

/// A mineable candidate: an isomorphic subsequence and how many successful occurrences it had in
/// the window. Enters the generic gate (§16.5) with agreement predicate = shadow-compare (§19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcedureCandidate {
    pub signature: Vec<TraceStep>,
    pub occurrences: u64,
    /// Distinct flow instances the candidate appeared in (evidence is never inflated by repetition
    /// within one flow, §13.2).
    pub distinct_flows: u64,
}

/// v0 mining parameters (§19).
#[derive(Debug, Clone, Copy)]
pub struct MiningParams {
    pub window_days: u32,
    pub min_occurrences: u64,
    pub min_len: usize,
    pub max_len: usize,
}

impl Default for MiningParams {
    fn default() -> Self {
        MiningParams {
            window_days: 30,
            min_occurrences: 5,
            min_len: 2,
            max_len: 6,
        }
    }
}

/// Mine isomorphic contiguous Call subsequences from successful in-window traces (§19). Returns
/// candidates meeting the occurrence threshold, longest-and-most-frequent first.
pub fn mine(traces: &[FlowTrace], params: &MiningParams, now_day: u32) -> Vec<ProcedureCandidate> {
    // signature → (total occurrences, set of flow indices it appeared in)
    let mut counts: BTreeMap<Vec<TraceStep>, (u64, std::collections::BTreeSet<usize>)> =
        BTreeMap::new();

    for (flow_idx, trace) in traces.iter().enumerate() {
        if !trace.success {
            continue; // only successful occurrences count (§19)
        }
        if now_day.saturating_sub(trace.day) > params.window_days {
            continue; // outside the 30-day window
        }
        let n = trace.steps.len();
        for len in params.min_len..=params.max_len {
            if len > n {
                break;
            }
            for start in 0..=(n - len) {
                let sig = trace.steps[start..start + len].to_vec();
                let entry = counts.entry(sig).or_insert((0, Default::default()));
                entry.0 += 1;
                entry.1.insert(flow_idx);
            }
        }
    }

    let mut out: Vec<ProcedureCandidate> = counts
        .into_iter()
        .filter(|(_, (occ, _))| *occ >= params.min_occurrences)
        .map(|(signature, (occurrences, flows))| ProcedureCandidate {
            signature,
            occurrences,
            distinct_flows: flows.len() as u64,
        })
        .collect();

    // Prefer longer, then more frequent — a maximal recurring procedure over its sub-fragments.
    out.sort_by(|a, b| {
        b.signature
            .len()
            .cmp(&a.signature.len())
            .then(b.occurrences.cmp(&a.occurrences))
    });
    out
}
