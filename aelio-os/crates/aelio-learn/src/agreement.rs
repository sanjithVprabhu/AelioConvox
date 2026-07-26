//! Per-class agreement predicates for the generic gate (§16.5). The gate's phases, evidence, tiers,
//! and lifecycle are identical across artifact classes; **only the agreement predicate varies**.
//! This module supplies the pathway and procedure predicates; the converter predicate is
//! `aelio_convert::gate::shadow_validate` (digest validation). All three feed the *same*
//! `aelio_convert::shadow_evidence` accumulator (over distinct inputs, §13.2).

use crate::procedures::TraceStep;

/// One shadow-phase turn for a **pathway** prototype (§18 gate integration). Shadow scores the new
/// prototype but takes the incumbent; evidence is the win rate **over turns where the incumbent
/// failed** — promotion is earned by addressing observed failures, not plausibility.
#[derive(Debug, Clone, Copy)]
pub struct PathwayTurn {
    pub incumbent_failed: bool,
    /// Would the new prototype have produced a successful outcome on this turn?
    pub new_would_succeed: bool,
}

/// Retrospective outcome match. Returns `None` for turns that carry no evidence (incumbent
/// succeeded — not a turn the challenger needs to win); `Some(agreed)` for evidence-bearing turns.
pub fn pathway_agreement(turn: &PathwayTurn) -> Option<bool> {
    if turn.incumbent_failed {
        Some(turn.new_would_succeed)
    } else {
        None
    }
}

/// Shadow-compare for a **procedure** (§19 gate integration): the miner predicts the Call sequence
/// while the flow executes the old way; agreement is exact match of predicted vs actual.
pub fn procedure_agreement(predicted: &[TraceStep], actual: &[TraceStep]) -> bool {
    predicted == actual
}
