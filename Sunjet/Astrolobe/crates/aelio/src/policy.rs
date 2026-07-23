//! Closed predicate grammar + Policy.Evaluate.
//!
//! Predicates may reference only: state, role, slot, env, tool, consent, budget,
//! evidence, flow_context. No function calls, no loops, no I/O.

use crate::contract::Predicate;
use crate::tenant::{PolicyEffect, PolicySpec};
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Evaluation context for policy predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyActionRisk {
    #[default]
    ReadOnly,
    EffectfulTool,
    StateTransition,
    FlowActivation,
    DocumentWrite,
    MemoryWrite,
    MemoryDelete,
}

#[derive(Debug, Clone, Default)]
pub struct PolicyCtx {
    pub state: Option<String>,
    pub role: Option<String>,
    pub tenant: Option<String>,
    pub segment: Option<String>,
    pub slots: IndexMap<String, Value>,
    pub env: IndexMap<String, Value>,
    pub tool: Option<String>,
    pub capability: Option<String>,
    pub consent: Option<bool>,
    pub budget: IndexMap<String, Value>,
    pub evidence: IndexMap<String, Value>,
    pub flow_context: IndexMap<String, Value>,
    pub transition: Option<String>,
    pub flow_id: Option<String>,
    /// Drives the fail-safe default when no authored policy matches.
    pub action_risk: PolicyActionRisk,
}

impl PolicyCtx {
    fn resolve_path(&self, path: &str) -> Option<serde_json::Value> {
        let (root, rest) = match path.split_once('.') {
            Some((a, b)) => (a, Some(b)),
            None => (path, None),
        };
        let base = match root {
            "state" => self.state.as_ref().map(|s| serde_json::json!(s)),
            "role" => self.role.as_ref().map(|s| serde_json::json!(s)),
            "tenant" => self.tenant.as_ref().map(|s| serde_json::json!(s)),
            "segment" => self.segment.as_ref().map(|s| serde_json::json!(s)),
            "tool" => self.tool.as_ref().map(|s| serde_json::json!(s)),
            "capability" => self.capability.as_ref().map(|s| serde_json::json!(s)),
            "consent" => self.consent.map(|b| serde_json::json!(b)),
            "transition" => self.transition.as_ref().map(|s| serde_json::json!(s)),
            "flow_id" => self.flow_id.as_ref().map(|s| serde_json::json!(s)),
            "slot" => {
                let key = rest.unwrap_or("");
                self.slots.get(key).map(value_to_json)
            }
            "env" => {
                let key = rest.unwrap_or("");
                self.env.get(key).map(value_to_json)
            }
            "budget" => {
                let key = rest.unwrap_or("");
                self.budget.get(key).map(value_to_json)
            }
            "evidence" => {
                let key = rest.unwrap_or("");
                self.evidence.get(key).map(value_to_json)
            }
            "flow_context" => {
                let key = rest.unwrap_or("");
                self.flow_context.get(key).map(value_to_json)
            }
            _ => None,
        };
        // For roots without subpath (state, role, ...), rest must be None.
        match root {
            "slot" | "env" | "budget" | "evidence" | "flow_context" => base,
            _ if rest.is_none() => base,
            _ => None,
        }
    }
}

fn value_to_json(v: &Value) -> serde_json::Value {
    crate::ops::pure::value_to_json(v)
}

/// Evaluate a closed predicate against PolicyCtx.
pub fn eval_predicate(pred: &Predicate, ctx: &PolicyCtx) -> bool {
    match pred {
        Predicate::True => true,
        Predicate::False => false,
        Predicate::Eq { path, value } => ctx.resolve_path(path).as_ref() == Some(value),
        Predicate::Ne { path, value } => ctx.resolve_path(path).as_ref() != Some(value),
        Predicate::In { path, values } => ctx
            .resolve_path(path)
            .map(|v| values.contains(&v))
            .unwrap_or(false),
        Predicate::Gt { path, value } => cmp_num(ctx, path, value, |a, b| a > b),
        Predicate::Gte { path, value } => cmp_num(ctx, path, value, |a, b| a >= b),
        Predicate::Lt { path, value } => cmp_num(ctx, path, value, |a, b| a < b),
        Predicate::Lte { path, value } => cmp_num(ctx, path, value, |a, b| a <= b),
        Predicate::Present { path } => {
            ctx.resolve_path(path).is_some()
                && ctx.resolve_path(path) != Some(serde_json::Value::Null)
        }
        Predicate::Absent { path } => {
            ctx.resolve_path(path).is_none()
                || ctx.resolve_path(path) == Some(serde_json::Value::Null)
        }
        Predicate::And { of } => of.iter().all(|p| eval_predicate(p, ctx)),
        Predicate::Or { of } => of.iter().any(|p| eval_predicate(p, ctx)),
        Predicate::Not { of } => !eval_predicate(of, ctx),
    }
}

fn cmp_num(
    ctx: &PolicyCtx,
    path: &str,
    value: &serde_json::Value,
    op: impl Fn(f64, f64) -> bool,
) -> bool {
    let left = ctx.resolve_path(path).and_then(|v| v.as_f64());
    let right = value.as_f64();
    match (left, right) {
        (Some(a), Some(b)) => op(a, b),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow {
        policy_id: Option<String>,
    },
    Deny {
        policy_id: String,
        reason_code: String,
        trace: Vec<String>,
    },
}

/// Policy.Evaluate — pure, never a model. Highest-priority matching Deny wins;
/// else any Allow. Unmatched reads are allowed; unmatched mutations are denied.
pub fn evaluate(policies: &[PolicySpec], ctx: &PolicyCtx) -> PolicyDecision {
    let mut matching: Vec<&PolicySpec> = policies
        .iter()
        .filter(|p| {
            subject_matches(p, ctx) && action_matches(p, ctx) && eval_predicate(&p.condition, ctx)
        })
        .collect();
    matching.sort_by_key(|p| -p.priority);

    let mut trace = Vec::new();
    for p in &matching {
        trace.push(format!("{}:{:?}", p.id, p.effect));
        if p.effect == PolicyEffect::Deny {
            return PolicyDecision::Deny {
                policy_id: p.id.clone(),
                reason_code: p.reason_code.clone(),
                trace,
            };
        }
    }
    if let Some(p) = matching.iter().find(|p| p.effect == PolicyEffect::Allow) {
        return PolicyDecision::Allow {
            policy_id: Some(p.id.clone()),
        };
    }
    if ctx.action_risk == PolicyActionRisk::ReadOnly {
        PolicyDecision::Allow { policy_id: None }
    } else {
        PolicyDecision::Deny {
            policy_id: "system.default_deny".into(),
            reason_code: "explicit_allow_required".into(),
            trace: vec![format!("unmatched:{:?}", ctx.action_risk)],
        }
    }
}

fn subject_matches(p: &PolicySpec, ctx: &PolicyCtx) -> bool {
    if let Some(ref s) = p.subject.state {
        if ctx.state.as_ref() != Some(s) {
            return false;
        }
    }
    if let Some(ref r) = p.subject.role {
        if ctx.role.as_ref() != Some(r) {
            return false;
        }
    }
    if let Some(ref t) = p.subject.tenant {
        if ctx.tenant.as_ref() != Some(t) {
            return false;
        }
    }
    if let Some(ref seg) = p.subject.segment {
        if ctx.segment.as_ref() != Some(seg) {
            return false;
        }
    }
    true
}

fn action_matches(p: &PolicySpec, ctx: &PolicyCtx) -> bool {
    if let Some(ref c) = p.action.capability {
        let matches = if c.ends_with('*') {
            let prefix = c.trim_end_matches('*').trim_end_matches('.');
            ctx.capability
                .as_deref()
                .is_some_and(|actual| actual == prefix || actual.starts_with(&format!("{prefix}.")))
        } else {
            ctx.capability.as_ref() == Some(c) || ctx.tool.as_ref() == Some(c)
        };
        if !matches {
            return false;
        }
    }
    if let Some(ref t) = p.action.tool_id {
        if ctx.tool.as_ref() != Some(t) {
            return false;
        }
    }
    if let Some(ref tr) = p.action.transition {
        if ctx.transition.as_ref() != Some(tr) {
            return false;
        }
    }
    if let Some(ref f) = p.action.flow_id {
        if ctx.flow_id.as_ref() != Some(f) {
            return false;
        }
    }
    true
}

/// Structural invariant wrapper: must wrap every Invoke.Call and State.ProposeTransition.
pub fn require_allow(policies: &[PolicySpec], ctx: &PolicyCtx) -> AelioResult<()> {
    match evaluate(policies, ctx) {
        PolicyDecision::Allow { .. } => Ok(()),
        PolicyDecision::Deny {
            policy_id,
            reason_code,
            ..
        } => Err(AelioError::new(
            ReasonCode::PolicyDenied,
            format!("denied by {policy_id}: {reason_code}"),
        )),
    }
}

/// Policy.Explain — predicate trace for audit.
pub fn explain(policies: &[PolicySpec], ctx: &PolicyCtx) -> Vec<String> {
    let mut out = Vec::new();
    for p in policies {
        let subj = subject_matches(p, ctx);
        let act = action_matches(p, ctx);
        let cond = eval_predicate(&p.condition, ctx);
        out.push(format!(
            "{}: subject={subj} action={act} condition={cond} effect={:?}",
            p.id, p.effect
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Predicate;
    use crate::tenant::{PolicyAction, PolicySubject};

    fn deny_unauth_write() -> PolicySpec {
        PolicySpec {
            id: "deny-unauth-write".into(),
            effect: PolicyEffect::Deny,
            subject: PolicySubject {
                state: Some("unauthenticated".into()),
                ..Default::default()
            },
            action: PolicyAction {
                capability: Some("orders.cancel".into()),
                ..Default::default()
            },
            condition: Predicate::True,
            reason_code: "auth_required".into(),
            priority: 100,
        }
    }

    #[test]
    fn deny_when_unauthenticated() {
        let policies = vec![deny_unauth_write()];
        let ctx = PolicyCtx {
            state: Some("unauthenticated".into()),
            capability: Some("orders.cancel".into()),
            ..Default::default()
        };
        assert!(matches!(
            evaluate(&policies, &ctx),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn unmatched_read_is_allowed() {
        let policies = vec![deny_unauth_write()];
        let ctx = PolicyCtx {
            state: Some("authenticated".into()),
            capability: Some("orders.cancel".into()),
            ..Default::default()
        };
        assert!(matches!(
            evaluate(&policies, &ctx),
            PolicyDecision::Allow { .. }
        ));
    }

    #[test]
    fn unmatched_effect_is_denied_but_explicit_allow_opens_it() {
        let mut ctx = PolicyCtx {
            capability: Some("orders.cancel".into()),
            action_risk: PolicyActionRisk::EffectfulTool,
            ..Default::default()
        };
        assert!(matches!(evaluate(&[], &ctx), PolicyDecision::Deny { .. }));

        let mut allow = deny_unauth_write();
        allow.effect = PolicyEffect::Allow;
        allow.subject = PolicySubject::default();
        ctx.state = Some("authenticated".into());
        assert!(matches!(
            evaluate(&[allow], &ctx),
            PolicyDecision::Allow { .. }
        ));
    }

    #[test]
    fn closed_grammar_state_eq() {
        let pred = Predicate::state_eq("unauthenticated");
        let ctx = PolicyCtx {
            state: Some("unauthenticated".into()),
            ..Default::default()
        };
        assert!(eval_predicate(&pred, &ctx));
    }

    #[test]
    fn exact_capability_policy_does_not_expand_to_prefix() {
        let mut policy = deny_unauth_write();
        policy.action.capability = Some("orders".into());
        let ctx = PolicyCtx {
            state: Some("unauthenticated".into()),
            capability: Some("orders.cancel".into()),
            ..Default::default()
        };
        assert!(matches!(
            evaluate(&[policy.clone()], &ctx),
            PolicyDecision::Allow { .. }
        ));
        policy.action.capability = Some("orders.*".into());
        assert!(matches!(
            evaluate(&[policy], &ctx),
            PolicyDecision::Deny { .. }
        ));
    }
}
