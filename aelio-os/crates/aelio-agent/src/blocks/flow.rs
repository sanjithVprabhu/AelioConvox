//! Flow control blocks: activate, advance, suspend, resume.
//! Resumed flows re-check policy and state — never trust pre-suspension auth.

use crate::contract::Predicate;
use crate::policy::{eval_predicate, evaluate, PolicyActionRisk, PolicyCtx, PolicyDecision};
use crate::tenant::{FlowEscape, FlowSpec, PolicySpec};
use crate::types::{AelioError, AelioResult, ReasonCode};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowInstance {
    pub flow_id: String,
    pub flow_version: String,
    pub current_step_idx: usize,
    pub slots: IndexMap<String, serde_json::Value>,
    pub attempts: u32,
    pub pinned_tool_versions: IndexMap<String, String>,
    pub pinned_prompt_hashes: IndexMap<String, String>,
    pub ttl_secs: Option<u64>,
    pub pending_step: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlowGateResult {
    /// No active flow — continue free-range turn.
    ContinueFree,
    /// Resume parked flow (pending_step present).
    Resume { instance: Box<FlowInstance> },
    /// Activate a new flow.
    Activate { flow_id: String },
}

/// O2: flow context outranks utterance semantics.
pub fn flow_gate(active: Option<&FlowInstance>) -> FlowGateResult {
    if let Some(inst) = active {
        if inst.pending_step.is_some() {
            return FlowGateResult::Resume {
                instance: Box::new(inst.clone()),
            };
        }
    }
    FlowGateResult::ContinueFree
}

/// Activation: hard preconditions first, then trigger surface (embedding/margin only breaks ties).
pub fn activate_flow(
    flow: &FlowSpec,
    policies: &[PolicySpec],
    ctx: &PolicyCtx,
    trigger_margin: f64,
) -> AelioResult<()> {
    for pred in &flow.activation.hard_preconditions {
        if !eval_predicate(pred, ctx) {
            return Err(AelioError::new(
                ReasonCode::Denied,
                "hard precondition failed",
            ));
        }
    }
    let mut pctx = ctx.clone();
    pctx.flow_id = Some(flow.id.clone());
    pctx.action_risk = PolicyActionRisk::FlowActivation;
    match evaluate(policies, &pctx) {
        PolicyDecision::Deny { reason_code, .. } => {
            return Err(AelioError::new(ReasonCode::PolicyDenied, reason_code));
        }
        PolicyDecision::Allow { .. } => {}
    }
    if trigger_margin < flow.activation.margin_threshold {
        return Err(AelioError::new(
            ReasonCode::Denied,
            format!(
                "trigger margin {trigger_margin} < {}",
                flow.activation.margin_threshold
            ),
        ));
    }
    Ok(())
}

pub fn start_instance(flow: &FlowSpec) -> FlowInstance {
    FlowInstance {
        flow_id: flow.id.clone(),
        flow_version: flow.version.clone(),
        current_step_idx: 0,
        slots: IndexMap::new(),
        attempts: 0,
        pinned_tool_versions: IndexMap::new(),
        pinned_prompt_hashes: IndexMap::new(),
        ttl_secs: flow.ttl_secs,
        pending_step: flow.steps.first().map(|s| s.id.clone()),
    }
}

pub fn suspend(
    instance: &mut FlowInstance,
    pending_step: &str,
    slots: IndexMap<String, serde_json::Value>,
) {
    instance.pending_step = Some(pending_step.into());
    instance.slots = slots;
}

/// Resume: re-check policy, state, and pinned versions.
///
/// A suspended instance pins the exact tool versions *and* prompt hashes it started with; a prompt
/// edit changes an ability's behavior just as a tool redeploy changes its shape, so both must be
/// re-verified before the flow runs against them. If either drifted, the flow's declared escape
/// fires rather than silently executing against changed semantics (decisions N1/N2/C6/F8).
pub fn resume_flow(
    instance: &FlowInstance,
    flow: &FlowSpec,
    policies: &[PolicySpec],
    ctx: &PolicyCtx,
    tool_versions_current: &IndexMap<String, String>,
    prompt_hashes_current: &IndexMap<String, String>,
) -> AelioResult<ResumeVerdict> {
    if flow.version != instance.flow_version {
        return Ok(ResumeVerdict::Escape(flow.escape.clone()));
    }
    for (tool, pinned) in &instance.pinned_tool_versions {
        match tool_versions_current.get(tool) {
            Some(cur) if cur == pinned => {}
            _ => return Ok(ResumeVerdict::Escape(flow.escape.clone())),
        }
    }
    for (ability, pinned) in &instance.pinned_prompt_hashes {
        match prompt_hashes_current.get(ability) {
            Some(cur) if cur == pinned => {}
            _ => return Ok(ResumeVerdict::Escape(flow.escape.clone())),
        }
    }
    let mut pctx = ctx.clone();
    pctx.flow_id = Some(flow.id.clone());
    pctx.action_risk = PolicyActionRisk::FlowActivation;
    if let PolicyDecision::Deny { .. } = evaluate(policies, &pctx) {
        return Ok(ResumeVerdict::Escape(flow.escape.clone()));
    }
    // re-check hard preconditions (state may have changed)
    for pred in &flow.activation.hard_preconditions {
        if !eval_predicate(pred, ctx) {
            return Ok(ResumeVerdict::Escape(flow.escape.clone()));
        }
    }
    Ok(ResumeVerdict::Continue)
}

#[derive(Debug, Clone)]
pub enum ResumeVerdict {
    Continue,
    Escape(FlowEscape),
}

pub fn advance(instance: &mut FlowInstance, flow: &FlowSpec) -> Option<String> {
    instance.current_step_idx += 1;
    if instance.current_step_idx >= flow.steps.len() {
        instance.pending_step = None;
        None
    } else {
        let id = flow.steps[instance.current_step_idx].id.clone();
        instance.pending_step = Some(id.clone());
        Some(id)
    }
}

pub fn step_postcondition_met(flow: &FlowSpec, step_id: &str, ctx: &PolicyCtx) -> bool {
    flow.steps
        .iter()
        .find(|s| s.id == step_id)
        .map(|s| eval_predicate(&s.postcondition, ctx))
        .unwrap_or(false)
}

/// Structural bar: learnable:false flows cannot be reordered by learning.
pub fn may_reorder(flow: &FlowSpec) -> bool {
    flow.learnable
}

pub fn trigger_surface_match(utterance: &str, surface: &[String]) -> f64 {
    let lower = utterance.to_lowercase();
    let mut best: f64 = 0.0;
    for t in surface {
        let t = t.to_lowercase();
        if lower == t {
            best = best.max(1.0);
        } else if lower.contains(&t) || t.contains(&lower) {
            best = best.max(0.85);
        }
    }
    best
}

pub fn always_true() -> Predicate {
    Predicate::True
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::{
        FlowActivation, FlowStep, PolicyAction, PolicyEffect, PolicySubject, Preemption,
        ViolationAction,
    };

    fn allow_effects() -> Vec<PolicySpec> {
        vec![PolicySpec {
            id: "allow-test-flow".into(),
            effect: PolicyEffect::Allow,
            subject: PolicySubject::default(),
            action: PolicyAction::default(),
            condition: Predicate::True,
            reason_code: "test".into(),
            priority: 1,
        }]
    }

    fn login_flow() -> FlowSpec {
        FlowSpec {
            id: "login".into(),
            version: "1".into(),
            name: "login".into(),
            activation: FlowActivation {
                hard_preconditions: vec![],
                trigger_surface: vec!["log in".into()],
                margin_threshold: 0.5,
            },
            learnable: false,
            preemption: Preemption::Hold,
            steps: vec![FlowStep {
                id: "await_otp".into(),
                intent: "verify otp".into(),
                postcondition: Predicate::True,
                admissible: vec!["auth.otp.verify".into()],
                on_violation: ViolationAction::Escape,
                suspendable: true,
            }],
            escape: FlowEscape::Escalate,
            terminal_states: vec!["authenticated".into()],
            ttl_secs: Some(300),
            max_attempts: 3,
            lowering: None,
        }
    }

    fn parked_instance(flow: &FlowSpec) -> FlowInstance {
        let mut instance = start_instance(flow);
        instance.pending_step = Some("await_otp".into());
        instance
            .pinned_tool_versions
            .insert("verify_otp".into(), "1".into());
        instance
            .pinned_prompt_hashes
            .insert("Understand.Extract".into(), "hash_v1".into());
        instance
    }

    #[test]
    fn resume_escapes_when_pinned_prompt_hash_drifts() {
        let flow = login_flow();
        let instance = parked_instance(&flow);
        let ctx = PolicyCtx::default();
        let policies = allow_effects();
        let tool_versions = IndexMap::from([("verify_otp".to_string(), "1".to_string())]);

        // Same prompt hash → continue.
        let current_ok =
            IndexMap::from([("Understand.Extract".to_string(), "hash_v1".to_string())]);
        assert!(matches!(
            resume_flow(
                &instance,
                &flow,
                &policies,
                &ctx,
                &tool_versions,
                &current_ok
            )
            .unwrap(),
            ResumeVerdict::Continue
        ));

        // Prompt was edited while parked → escape, never run against changed semantics.
        let current_drift =
            IndexMap::from([("Understand.Extract".to_string(), "hash_v2".to_string())]);
        assert!(matches!(
            resume_flow(
                &instance,
                &flow,
                &policies,
                &ctx,
                &tool_versions,
                &current_drift
            )
            .unwrap(),
            ResumeVerdict::Escape(_)
        ));
    }
}
