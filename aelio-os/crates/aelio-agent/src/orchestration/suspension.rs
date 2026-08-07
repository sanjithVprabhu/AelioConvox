//! Mid-TaskGraph suspension state for needs_info / needs_approval gates.

use crate::orchestration::wavefront::{ExecutorState, LedgerEntry};
use aelio_sol::TaskGraph;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// A graph clarification remains live for five minutes unless its caller supplies a stricter
/// deadline. This bounds stale user input without making an expired continuation executable.
pub const DEFAULT_GRAPH_SUSPENSION_TTL_MS: i64 = 300_000;
pub const DEFAULT_GRAPH_SUSPENSION_MAX_RETRIES: u8 = 3;

/// Persisted state when orchestration parks mid-graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GraphSuspension {
    /// The turn that originally parked this graph. Resume audit records link back to this id.
    pub turn_id: String,
    pub user_id: String,
    pub graph: TaskGraph,
    pub slots: IndexMap<String, crate::types::Value>,
    pub executor_state: SuspendedExecutorSnapshot,
    pub pending_node_id: String,
    pub reason: GraphSuspensionReason,
    #[serde(default)]
    pub retry_count: u8,
    #[serde(default = "default_max_retries")]
    pub max_retries: u8,
    #[serde(default)]
    pub opened_at_ms: i64,
    #[serde(default)]
    pub expires_at_ms: i64,
}

fn default_max_retries() -> u8 {
    DEFAULT_GRAPH_SUSPENSION_MAX_RETRIES
}

impl GraphSuspension {
    pub fn awaiting_info(
        turn_id: String,
        user_id: String,
        graph: TaskGraph,
        slots: IndexMap<String, crate::types::Value>,
        executor_state: SuspendedExecutorSnapshot,
        pending_node_id: String,
        missing: Vec<String>,
        now_ms: i64,
    ) -> Self {
        Self {
            turn_id,
            user_id,
            graph,
            slots,
            executor_state,
            pending_node_id,
            reason: GraphSuspensionReason::AwaitingInfo { missing },
            retry_count: 0,
            max_retries: DEFAULT_GRAPH_SUSPENSION_MAX_RETRIES,
            opened_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(DEFAULT_GRAPH_SUSPENSION_TTL_MS),
        }
    }

    pub fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms > 0 && now_ms >= self.expires_at_ms
    }

    pub fn can_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphSuspensionReason {
    AwaitingInfo { missing: Vec<String> },
    AwaitingConfirmation { tool_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuspendedExecutorSnapshot {
    pub ledger: Vec<LedgerEntry>,
    pub completed: Vec<String>,
    pub outputs: IndexMap<String, crate::types::Value>,
}

impl From<&ExecutorState> for SuspendedExecutorSnapshot {
    fn from(state: &ExecutorState) -> Self {
        Self {
            ledger: state.ledger.clone(),
            completed: state.completed.iter().cloned().collect(),
            outputs: state
                .outputs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

impl SuspendedExecutorSnapshot {
    pub fn restore(&self) -> ExecutorState {
        let mut state = ExecutorState::from_ledger(self.ledger.clone());
        for id in &self.completed {
            state.completed.insert(id.clone());
        }
        for (key, value) in &self.outputs {
            state.outputs.insert(key.clone(), value.clone());
        }
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_sol::{JoinSpec, TaskBudget, TaskGraph, TaskNode};

    #[test]
    fn suspension_roundtrip_snapshot() {
        let graph = TaskGraph {
            root_goal: "test".into(),
            nodes: vec![TaskNode {
                id: "a".into(),
                goal: "g".into(),
                rationale: "r".into(),
                strategy_hint: vec![],
                inputs: vec![],
                outputs: vec![],
                depends_on: vec![],
                budget: TaskBudget {
                    max_steps: 1,
                    max_tokens: 100,
                    max_ms: 1000,
                },
                max_refinements: 0,
            }],
            join_spec: JoinSpec::LastNode {
                node_id: "a".into(),
                field: "result".into(),
            },
        };
        let state = ExecutorState::default();
        let snap = SuspendedExecutorSnapshot::from(&state);
        let suspension = GraphSuspension {
            turn_id: "t1".into(),
            user_id: "u1".into(),
            graph,
            slots: IndexMap::new(),
            executor_state: snap,
            pending_node_id: "a".into(),
            reason: GraphSuspensionReason::AwaitingInfo {
                missing: vec!["values".into()],
            },
            retry_count: 0,
            max_retries: DEFAULT_GRAPH_SUSPENSION_MAX_RETRIES,
            opened_at_ms: 0,
            expires_at_ms: 0,
        };
        assert_eq!(suspension.executor_state.restore().ledger.len(), 0);
    }
}
