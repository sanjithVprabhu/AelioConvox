//! Wavefront scheduler and per-turn idempotency ledger (port of TS harness resolver/executor).

use crate::tenant::ToolSpec;
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use aelio_sol::{SlotRef, TaskGraph, TaskNode};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One executed step in the turn ledger.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerEntry {
    pub node_id: String,
    pub tool_id: String,
    pub args_hash: String,
    pub status: LedgerStatus,
    pub result: Value,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LedgerStatus {
    Success,
    Error,
}

#[derive(Debug, Default, Clone)]
pub struct ExecutorState {
    pub ledger: Vec<LedgerEntry>,
    pub completed: BTreeSet<String>,
    pub outputs: BTreeMap<String, Value>,
}

impl ExecutorState {
    pub fn from_ledger(existing: Vec<LedgerEntry>) -> Self {
        let mut state = Self::default();
        for entry in existing {
            if entry.status == LedgerStatus::Success {
                state.completed.insert(entry.node_id.clone());
                state.outputs.insert(entry.node_id.clone(), entry.result.clone());
            }
            state.ledger.push(entry);
        }
        state
    }
}

/// Stable hash of canonicalized args for idempotency.
///
/// BLAKE3 over §4.3-canonical SolValue form — not serde_json (F-032 / §4.3).
pub fn hash_args(args: &IndexMap<String, Value>) -> String {
    use aelio_sol::SolValue;
    use std::collections::BTreeMap;
    let mut keys: Vec<&String> = args.keys().collect();
    keys.sort();
    let mut canonical = BTreeMap::new();
    for key in keys {
        let value = args
            .get(key)
            .expect("key collected from same map");
        canonical.insert(
            key.clone(),
            crate::ops::pure::value_to_sol(value).unwrap_or(SolValue::Null),
        );
    }
    aelio_sol::value_hash(&SolValue::Map(canonical))
}

/// Stable per-effect idempotency identity. Deliberately excludes user/tool/argument data:
/// retries keep the same caller-supplied turn key, while every distinct effect gets its own
/// monotonic sequence and map children additionally carry their element position.
pub fn effect_idempotency_key(
    turn_key: &str,
    effect_seq: u64,
    element_index: Option<u64>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in [
        b"aelio.effect-idempotency.v1".as_slice(),
        turn_key.as_bytes(),
        &effect_seq.to_be_bytes(),
    ] {
        hasher.update(&(part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    match element_index {
        Some(index) => {
            hasher.update(&[1]);
            hasher.update(&index.to_be_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// Compute the next wave of ready nodes from a TaskGraph.
pub fn next_wave<'a>(
    graph: &'a TaskGraph,
    completed: &BTreeSet<String>,
) -> AelioResult<Vec<&'a TaskNode>> {
    let waves = graph
        .waves()
        .map_err(|e| AelioError::new(ReasonCode::Validation, e.to_string()))?;
    for wave in waves {
        let pending: Vec<&TaskNode> = wave
            .into_iter()
            .filter(|node| !completed.contains(&node.id))
            .collect();
        if !pending.is_empty() {
            return Ok(pending);
        }
    }
    Ok(vec![])
}

/// Resolve node inputs from slot namespace and prior outputs.
pub fn resolve_node_inputs(
    node: &TaskNode,
    slots: &IndexMap<String, Value>,
    outputs: &BTreeMap<String, Value>,
) -> AelioResult<IndexMap<String, Value>> {
    let mut resolved = IndexMap::new();
    for (idx, input) in node.inputs.iter().enumerate() {
        match input {
            SlotRef::Slot { name } => {
                let value = slots.get(name).cloned().ok_or_else(|| {
                    AelioError::new(ReasonCode::Missing, format!("missing slot `{name}`"))
                })?;
                resolved.insert(name.clone(), value);
            }
            SlotRef::Literal { value } => {
                resolved.insert(format!("literal_{idx}"), json_to_value(value));
            }
            SlotRef::NodeOutput { node_id, field } => {
                let producer = outputs.get(node_id).ok_or_else(|| {
                    AelioError::new(
                        ReasonCode::Validation,
                        format!("missing output from node `{node_id}`"),
                    )
                })?;
                let value = extract_field(producer, field)?;
                resolved.insert(field.clone(), value);
            }
        }
    }
    Ok(resolved)
}

fn extract_field(value: &Value, field: &str) -> AelioResult<Value> {
    if let Value::Map(map) = value {
        map.get(field)
            .cloned()
            .ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Validation,
                    format!("field `{field}` not found in output map"),
                )
            })
    } else {
        Err(AelioError::new(
            ReasonCode::Validation,
            "expected map output",
        ))
    }
}

fn json_to_value(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::Str(s.clone()),
        serde_json::Value::Array(items) => {
            Value::List(items.iter().map(json_to_value).collect())
        }
        serde_json::Value::Object(map) => {
            let mut out = IndexMap::new();
            for (k, v) in map {
                out.insert(k.clone(), json_to_value(v));
            }
            Value::Map(out)
        }
    }
}

/// Outcome of invoking one node's tool. A gated client tool may stop to ask the user for a
/// parameter it could not bind, in which case nothing is recorded in the ledger.
#[derive(Debug, Clone)]
pub enum NodeInvocation {
    Value(Value),
    NeedUser {
        missing: Vec<String>,
        question: String,
    },
}

/// Invoke a tool for a node, checking idempotency ledger first.
pub fn invoke_node_tool(
    state: &mut ExecutorState,
    node: &TaskNode,
    tool: &ToolSpec,
    args: IndexMap<String, Value>,
    invoke: impl FnOnce(&ToolSpec, &IndexMap<String, Value>) -> AelioResult<NodeInvocation>,
) -> AelioResult<NodeInvocation> {
    let args_hash = hash_args(&args);
    if let Some(existing) = state.ledger.iter().find(|e| {
        e.node_id == node.id && e.args_hash == args_hash && e.status == LedgerStatus::Success
    }) {
        return Ok(NodeInvocation::Value(existing.result.clone()));
    }
    let started = std::time::Instant::now();
    let result = match invoke(tool, &args)? {
        NodeInvocation::Value(value) => value,
        need_user @ NodeInvocation::NeedUser { .. } => return Ok(need_user),
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    state.ledger.push(LedgerEntry {
        node_id: node.id.clone(),
        tool_id: tool.id.clone(),
        args_hash,
        status: LedgerStatus::Success,
        result: result.clone(),
        duration_ms,
    });
    state.completed.insert(node.id.clone());
    state.outputs.insert(node.id.clone(), result.clone());
    Ok(NodeInvocation::Value(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_sol::{JoinSpec, TaskBudget, TaskGraph, TaskNode};

    #[test]
    fn next_wave_returns_first_pending_nodes() {
        let graph = TaskGraph {
            root_goal: "test".into(),
            nodes: vec![
                TaskNode {
                    id: "a".into(),
                    goal: "A".into(),
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
                },
                TaskNode {
                    id: "b".into(),
                    goal: "B".into(),
                    rationale: "r".into(),
                    strategy_hint: vec![],
                    inputs: vec![],
                    outputs: vec![],
                    depends_on: vec!["a".into()],
                    budget: TaskBudget {
                        max_steps: 1,
                        max_tokens: 100,
                        max_ms: 1000,
                    },
                    max_refinements: 0,
                },
            ],
            join_spec: JoinSpec::LastNode {
                node_id: "b".into(),
                field: "result".into(),
            },
        };
        let wave = next_wave(&graph, &BTreeSet::new()).expect("wave");
        assert_eq!(wave.len(), 1);
        assert_eq!(wave[0].id, "a");
    }

    /// D3: an idempotent downstream client must see one identity per mapped element, rather than
    /// deduplicating all five effects into the first call's key.
    #[test]
    fn five_map_elements_receive_distinct_effect_keys() {
        let mut deduped_writes = BTreeSet::new();
        for element_index in 0..5 {
            let key = effect_idempotency_key("turn-k", 7, Some(element_index));
            deduped_writes.insert(key);
        }
        assert_eq!(deduped_writes.len(), 5);
        assert_ne!(
            effect_idempotency_key("turn-k", 7, None),
            effect_idempotency_key("turn-k", 7, Some(0))
        );
    }
}
