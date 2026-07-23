//! State.* — lifecycle position and transitions.

use crate::policy::{require_allow, PolicyActionRisk, PolicyCtx};
use crate::tenant::{PolicySpec, StateSpec};
use crate::types::{AelioError, AelioResult, ReasonCode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateNode {
    pub id: String,
    pub name: String,
    pub permission_envelope: Vec<String>,
    pub direction_target: Option<String>,
}

pub fn read(states: &[StateSpec], current_id: &str) -> AelioResult<StateNode> {
    states
        .iter()
        .find(|s| s.id == current_id)
        .map(|s| StateNode {
            id: s.id.clone(),
            name: s.name.clone(),
            permission_envelope: s.permission_envelope.clone(),
            direction_target: s.direction.as_ref().map(|d| d.target.clone()),
        })
        .ok_or_else(|| AelioError::new(ReasonCode::NotFound, format!("state {current_id}")))
}

pub fn direction(states: &[StateSpec], current_id: &str) -> Option<String> {
    states
        .iter()
        .find(|s| s.id == current_id)
        .and_then(|s| s.direction.as_ref().map(|d| d.target.clone()))
}

pub fn reachable(states: &[StateSpec], from: &str) -> Vec<String> {
    states
        .iter()
        .find(|s| s.id == from)
        .map(|s| s.exit_edges.iter().map(|e| e.to.clone()).collect())
        .unwrap_or_default()
}

/// Propose transition — policy invariant wraps this structurally.
pub fn propose_transition(
    states: &[StateSpec],
    policies: &[PolicySpec],
    current: &str,
    target: &str,
    evidence_ok: bool,
    policy_ctx: &PolicyCtx,
) -> AelioResult<String> {
    let mut ctx = policy_ctx.clone();
    ctx.transition = Some(format!("{current}->{target}"));
    ctx.state = Some(current.into());
    ctx.action_risk = PolicyActionRisk::StateTransition;
    require_allow(policies, &ctx)?;

    let state = states
        .iter()
        .find(|s| s.id == current)
        .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "current state"))?;

    let edge = state
        .exit_edges
        .iter()
        .find(|e| e.to == target)
        .ok_or_else(|| {
            AelioError::new(
                ReasonCode::Denied,
                format!("no exit edge {current} -> {target}"),
            )
        })?;

    if edge.evidence_required && !evidence_ok {
        return Err(AelioError::new(
            ReasonCode::Missing,
            "transition requires evidence",
        ));
    }

    // Guard evaluated against policy ctx
    if !crate::policy::eval_predicate(&edge.guard, &ctx) {
        return Err(AelioError::new(
            ReasonCode::Denied,
            "exit edge guard failed",
        ));
    }

    Ok(target.to_string())
}
