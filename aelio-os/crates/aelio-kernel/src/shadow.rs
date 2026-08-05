//! Phase 4.4 — Shadow Conductor vs live agent turn path.
//!
//! The live `/v1/turns` authority is unchanged. After (or beside) the agent turn,
//! we classify what the agent did, run deterministic OS Conductor selection on the
//! same utterance, and record agreement. Disagreements are observations — never
//! overrides.

use crate::harness_syscalls::{decide_deterministic, DeterministicConductorDecision};
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store, StoreError};
use serde::{Deserialize, Serialize};

pub const SHADOW_TABLE: &str = "conductor_shadow_v1";

/// Normalized route labels shared by agent and OS planes for comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowRoute {
    QuickReply,
    UnderstandIntent,
    WaitForUser,
    MemoryAttach,
    SpawnAverage,
    Escalate,
    /// Agent continued a parked/waiting child without Conductor re-select.
    AgentResume,
    /// Could not classify agent path from steps.
    Unknown,
}

impl ShadowRoute {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::QuickReply => "quick_reply",
            Self::UnderstandIntent => "understand_intent",
            Self::WaitForUser => "wait_for_user",
            Self::MemoryAttach => "memory_attach",
            Self::SpawnAverage => "spawn_average",
            Self::Escalate => "escalate",
            Self::AgentResume => "agent_resume",
            Self::Unknown => "unknown",
        }
    }
}

impl From<DeterministicConductorDecision> for ShadowRoute {
    fn from(d: DeterministicConductorDecision) -> Self {
        match d {
            DeterministicConductorDecision::QuickReplyGreeting
            | DeterministicConductorDecision::QuickReply => Self::QuickReply,
            DeterministicConductorDecision::UnderstandIntent => Self::UnderstandIntent,
            DeterministicConductorDecision::SpawnWaitForUser
            | DeterministicConductorDecision::WaitForUser => Self::WaitForUser,
            DeterministicConductorDecision::MemoryAttach => Self::MemoryAttach,
            DeterministicConductorDecision::SpawnAverage => Self::SpawnAverage,
            DeterministicConductorDecision::Escalate => Self::Escalate,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShadowObservation {
    pub turn_id: String,
    pub tenant_id: String,
    pub subject_id: String,
    pub utterance: String,
    pub agent_route: ShadowRoute,
    pub os_route: ShadowRoute,
    pub agree: bool,
    /// True when comparison is informative (both sides classifiable).
    pub comparable: bool,
    pub agent_select_detail: Option<String>,
    pub used_propose_path: bool,
    pub agent_suspended: bool,
}

/// Classify agent turn steps into a shadow route.
///
/// Looks for `Conductor.Select harness=…` first; falls back to ProposePath /
/// wait resume markers.
pub fn classify_agent_steps(
    steps: &[(String, String)],
    suspended: bool,
) -> (ShadowRoute, Option<String>, bool) {
    let mut select_detail = None;
    let mut used_propose = false;
    let mut selected: Option<ShadowRoute> = None;
    let mut saw_wait_resume = false;

    for (name, detail) in steps {
        if name == "Conductor.Select" {
            select_detail = Some(detail.clone());
            selected = Some(parse_select_detail(detail));
        }
        if name == "ProposePath" || detail.contains("ProposePath") {
            used_propose = true;
        }
        if name == "Harness.wait_for_user.resume"
            || (name == "Harness.Stack" && detail.contains("stay:") && detail.contains("waiting=true"))
        {
            // stay on waiting child — only treat as resume if no Conductor.Select this turn
            if selected.is_none() {
                saw_wait_resume = true;
            }
        }
        if name.contains("wait_for_user.resume") {
            saw_wait_resume = true;
        }
    }

    if let Some(route) = selected {
        return (route, select_detail, used_propose);
    }
    if used_propose {
        return (ShadowRoute::Escalate, select_detail, true);
    }
    if saw_wait_resume || suspended {
        // suspended without select often means wait parked this turn after select;
        // if no select and suspended, still Unknown unless resume marker.
        if saw_wait_resume {
            return (ShadowRoute::AgentResume, select_detail, used_propose);
        }
    }
    (ShadowRoute::Unknown, select_detail, used_propose)
}

fn parse_select_detail(detail: &str) -> ShadowRoute {
    // "harness=quick_reply — …"
    let harness = detail
        .strip_prefix("harness=")
        .and_then(|rest| rest.split(" — ").next())
        .unwrap_or(detail)
        .trim();
    match harness {
        "quick_reply" => ShadowRoute::QuickReply,
        "understand_intent" => ShadowRoute::UnderstandIntent,
        "wait_for_user" => ShadowRoute::WaitForUser,
        "memory_attach" => ShadowRoute::MemoryAttach,
        "escalate" => ShadowRoute::Escalate,
        other if other.contains("average") => ShadowRoute::SpawnAverage,
        _ => ShadowRoute::Unknown,
    }
}

/// Compare OS deterministic decision to agent classification.
pub fn observe_shadow(
    turn_id: &str,
    tenant_id: &str,
    subject_id: &str,
    utterance: &str,
    agent_steps: &[(String, String)],
    agent_suspended: bool,
) -> ShadowObservation {
    let (agent_route, select_detail, used_propose) =
        classify_agent_steps(agent_steps, agent_suspended);
    let os_decision = decide_deterministic("user.message", utterance);
    let os_route = ShadowRoute::from(os_decision);

    let comparable = !matches!(agent_route, ShadowRoute::Unknown | ShadowRoute::AgentResume);
    let agree = comparable && routes_agree(&agent_route, &os_route);

    ShadowObservation {
        turn_id: turn_id.into(),
        tenant_id: tenant_id.into(),
        subject_id: subject_id.into(),
        utterance: utterance.into(),
        agent_route,
        os_route,
        agree,
        comparable,
        agent_select_detail: select_detail,
        used_propose_path: used_propose,
        agent_suspended,
    }
}

fn routes_agree(agent: &ShadowRoute, os: &ShadowRoute) -> bool {
    if agent == os {
        return true;
    }
    // Soft agree: OS named a pure compound while the agent cold-escalated.
    matches!(
        (agent, os),
        (ShadowRoute::Escalate, ShadowRoute::SpawnAverage)
    )
}

/// Persist shadow observation (append-only by turn_id).
pub fn store_shadow(
    store: &mut dyn Store,
    obs: &ShadowObservation,
) -> Result<PutIfAbsent, StoreError> {
    let json = serde_json::to_string(obs)
        .map_err(|e| StoreError::Internal(format!("shadow serialize: {e}")))?;
    let value = SolValue::map([
        ("kind", SolValue::str("conductor_shadow_v1")),
        ("json", SolValue::str(json)),
        ("agree", SolValue::Bool(obs.agree)),
        ("comparable", SolValue::Bool(obs.comparable)),
        ("agent_route", SolValue::str(obs.agent_route.as_str())),
        ("os_route", SolValue::str(obs.os_route.as_str())),
    ]);
    store.put_if_absent(&obs.tenant_id, SHADOW_TABLE, &obs.turn_id, value)
}

/// One-line detail for TurnTraceStep.
pub fn shadow_trace_detail(obs: &ShadowObservation) -> String {
    format!(
        "agent={} os={} agree={} comparable={} propose={}",
        obs.agent_route.as_str(),
        obs.os_route.as_str(),
        obs.agree,
        obs.comparable,
        obs.used_propose_path
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_store::MemoryStore;

    #[test]
    fn greeting_agrees_quick_reply() {
        let steps = vec![(
            "Conductor.Select".into(),
            "harness=quick_reply — Answer briefly".into(),
        )];
        let obs = observe_shadow("t1", "tenant", "u1", "hello", &steps, false);
        assert_eq!(obs.agent_route, ShadowRoute::QuickReply);
        assert_eq!(obs.os_route, ShadowRoute::QuickReply);
        assert!(obs.agree);
        assert!(obs.comparable);
    }

    #[test]
    fn help_me_agrees_understand() {
        let steps = vec![(
            "Conductor.Select".into(),
            "harness=understand_intent — Clarify".into(),
        )];
        let obs = observe_shadow(
            "t2",
            "tenant",
            "u1",
            "help me figure out what to do next",
            &steps,
            false,
        );
        assert_eq!(obs.agent_route, ShadowRoute::UnderstandIntent);
        assert_eq!(obs.os_route, ShadowRoute::UnderstandIntent);
        assert!(obs.agree);
    }

    #[test]
    fn propose_path_classifies_as_escalate() {
        let steps = vec![("ProposePath".into(), "cold tools".into())];
        let (route, _, used) = classify_agent_steps(&steps, false);
        assert_eq!(route, ShadowRoute::Escalate);
        assert!(used);
    }

    #[test]
    fn shadow_store_dedupes_by_turn_id() {
        let mut store = MemoryStore::new();
        let steps = vec![(
            "Conductor.Select".into(),
            "harness=quick_reply — x".into(),
        )];
        let obs = observe_shadow("turn-9", "t", "u", "hi", &steps, false);
        let first = store_shadow(&mut store, &obs).unwrap();
        assert!(matches!(first, PutIfAbsent::Inserted { .. }));
        let second = store_shadow(&mut store, &obs).unwrap();
        assert!(matches!(second, PutIfAbsent::Existing(_)));
    }
}
