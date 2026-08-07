//! Harness v2 orchestration contracts — TaskGraph, TaskNode, evaluation verdicts.
//!
//! These are agent-layer planning artifacts (F-027). Each node lowers to Sol before kernel
//! execution. Canonical hashing uses the same discipline as §4.3 bags.

use crate::canonical::to_bytes;
use crate::hash::value_hash;
use crate::value::SolValue;
use crate::SolError;
use serde::{Deserialize, Serialize};
// BTree, not Hash: `waves()` iterates these to decide same-wave node ordering, and that ordering
// is observable (ledger sequencing, replay `bag_hash`). A HashSet/HashMap's iteration order is
// randomized per-process, which would make wavefront ordering non-deterministic across runs of
// the identical graph — see `FLAGS.md` F-032 and this crate's `clippy.toml`.
use std::collections::{BTreeMap, BTreeSet};

/// Per-node execution budget carved from the parent turn budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBudget {
    pub max_steps: u32,
    pub max_tokens: u32,
    pub max_ms: u64,
}

impl TaskBudget {
    pub fn fits_within(&self, parent: &TaskBudget) -> bool {
        self.max_steps <= parent.max_steps
            && self.max_tokens <= parent.max_tokens
            && self.max_ms <= parent.max_ms
    }

    pub fn default_turn() -> Self {
        Self {
            max_steps: 12,
            max_tokens: 30_000,
            max_ms: 60_000,
        }
    }
}

/// Reference to an input value for a task node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlotRef {
    Slot { name: String },
    Literal { value: serde_json::Value },
    NodeOutput { node_id: String, field: String },
}

/// Typed output shape a node must produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotSpec {
    pub name: String,
    pub ty: String,
}

/// A single sub-agent task within a TaskGraph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: String,
    pub goal: String,
    pub rationale: String,
    #[serde(default)]
    pub strategy_hint: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<SlotRef>,
    #[serde(default)]
    pub outputs: Vec<SlotSpec>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub budget: TaskBudget,
    #[serde(default)]
    pub max_refinements: u8,
}

/// How leaf node outputs combine into the parent answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JoinSpec {
    LastNode { node_id: String, field: String },
    MergeMap { node_fields: Vec<(String, String)> },
}

/// Verified task decomposition artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskGraph {
    pub root_goal: String,
    pub nodes: Vec<TaskNode>,
    pub join_spec: JoinSpec,
}

/// Complexity classification for orchestration routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplexityClass {
    Atomic,
    Composite,
    Open,
}

/// Typed evaluation verdict from the refinement gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalVerdict {
    pub pass: bool,
    pub confidence: f32,
    pub failure_reason: EvalFailureReason,
    #[serde(default)]
    pub repair_hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalFailureReason {
    None,
    ShapeMismatch,
    SemanticMismatch,
    BudgetExceeded,
    ToolError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskGraphValidationError {
    EmptyGraph,
    DuplicateNodeId(String),
    UnknownDependency { node_id: String, depends_on: String },
    CycleDetected,
    UnresolvedInput { node_id: String, detail: String },
    BudgetOverrun { node_id: String },
    InvalidJoinNode(String),
    EmptyGoal,
}

impl std::fmt::Display for TaskGraphValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyGraph => write!(f, "task graph has no nodes"),
            Self::DuplicateNodeId(id) => write!(f, "duplicate node id `{id}`"),
            Self::UnknownDependency {
                node_id,
                depends_on,
            } => {
                write!(f, "node `{node_id}` depends on unknown `{depends_on}`")
            }
            Self::CycleDetected => write!(f, "task graph contains a cycle"),
            Self::UnresolvedInput { node_id, detail } => {
                write!(f, "node `{node_id}` has unresolved input: {detail}")
            }
            Self::BudgetOverrun { node_id } => {
                write!(f, "node `{node_id}` budget exceeds parent remainder")
            }
            Self::InvalidJoinNode(id) => write!(f, "join_spec references unknown node `{id}`"),
            Self::EmptyGoal => write!(f, "root_goal must be non-empty"),
        }
    }
}

impl TaskGraph {
    /// Materialize the graph as a program-free [`SolValue`] for §4.3 hashing.
    pub fn to_sol_value(&self) -> SolValue {
        let mut nodes: Vec<&TaskNode> = self.nodes.iter().collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        SolValue::map([
            ("join_spec", join_spec_to_sol(&self.join_spec)),
            (
                "nodes",
                SolValue::List(
                    nodes
                        .into_iter()
                        .map(|node| task_node_to_sol(node).expect("task node must canonicalize"))
                        .collect(),
                ),
            ),
            ("root_goal", SolValue::str(self.root_goal.clone())),
        ])
    }

    /// Canonical byte form for promotion evidence (§4.3 — single serialiser via [`SolValue`]).
    pub fn canonical_bytes(&self) -> Vec<u8> {
        to_bytes(&self.to_sol_value())
    }

    /// Canonical BLAKE3 hash over the graph for promotion evidence.
    pub fn canonical_hash(&self) -> String {
        value_hash(&self.to_sol_value())
    }

    /// Validate structure: acyclic DAG, resolved refs, budgets within parent.
    pub fn validate(&self, parent_budget: &TaskBudget) -> Result<(), TaskGraphValidationError> {
        if self.root_goal.trim().is_empty() {
            return Err(TaskGraphValidationError::EmptyGoal);
        }
        if self.nodes.is_empty() {
            return Err(TaskGraphValidationError::EmptyGraph);
        }

        let ids: BTreeSet<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        if ids.len() != self.nodes.len() {
            for node in &self.nodes {
                if self.nodes.iter().filter(|n| n.id == node.id).count() > 1 {
                    return Err(TaskGraphValidationError::DuplicateNodeId(node.id.clone()));
                }
            }
        }

        for node in &self.nodes {
            if !node.budget.fits_within(parent_budget) {
                return Err(TaskGraphValidationError::BudgetOverrun {
                    node_id: node.id.clone(),
                });
            }
            for dep in &node.depends_on {
                if !ids.contains(dep.as_str()) {
                    return Err(TaskGraphValidationError::UnknownDependency {
                        node_id: node.id.clone(),
                        depends_on: dep.clone(),
                    });
                }
            }
            for input in &node.inputs {
                if let SlotRef::NodeOutput { node_id, field } = input {
                    if !ids.contains(node_id.as_str()) {
                        return Err(TaskGraphValidationError::UnresolvedInput {
                            node_id: node.id.clone(),
                            detail: format!("unknown producer node `{node_id}`"),
                        });
                    }
                    if field.trim().is_empty() {
                        return Err(TaskGraphValidationError::UnresolvedInput {
                            node_id: node.id.clone(),
                            detail: "empty output field name".into(),
                        });
                    }
                }
            }
        }

        if has_cycle(&self.nodes) {
            return Err(TaskGraphValidationError::CycleDetected);
        }

        match &self.join_spec {
            JoinSpec::LastNode { node_id, field } => {
                if !ids.contains(node_id.as_str()) {
                    return Err(TaskGraphValidationError::InvalidJoinNode(node_id.clone()));
                }
                if field.trim().is_empty() {
                    return Err(TaskGraphValidationError::UnresolvedInput {
                        node_id: node_id.clone(),
                        detail: "join field must be non-empty".into(),
                    });
                }
            }
            JoinSpec::MergeMap { node_fields } => {
                for (node_id, field) in node_fields {
                    if !ids.contains(node_id.as_str()) {
                        return Err(TaskGraphValidationError::InvalidJoinNode(node_id.clone()));
                    }
                    if field.trim().is_empty() {
                        return Err(TaskGraphValidationError::UnresolvedInput {
                            node_id: node_id.clone(),
                            detail: "merge field must be non-empty".into(),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    /// Topological waves for wavefront execution (independent nodes per wave).
    pub fn waves(&self) -> Result<Vec<Vec<&TaskNode>>, TaskGraphValidationError> {
        self.validate(&TaskBudget::default_turn())?;
        let index: BTreeMap<&str, &TaskNode> =
            self.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        // BTreeSet, not HashSet: `ready` below is built by iterating `remaining` directly, and
        // that order becomes the order nodes are pushed into this wave.
        let mut remaining: BTreeSet<&str> = index.keys().copied().collect();
        let mut completed: BTreeSet<&str> = BTreeSet::new();
        let mut waves = Vec::new();

        while !remaining.is_empty() {
            let ready: Vec<&TaskNode> = remaining
                .iter()
                .filter_map(|id| index.get(id))
                .filter(|node| {
                    node.depends_on
                        .iter()
                        .all(|d| completed.contains(d.as_str()))
                })
                .copied()
                .collect();
            if ready.is_empty() {
                return Err(TaskGraphValidationError::CycleDetected);
            }
            for node in &ready {
                remaining.remove(node.id.as_str());
                completed.insert(node.id.as_str());
            }
            waves.push(ready);
        }
        Ok(waves)
    }
}

fn has_cycle(nodes: &[TaskNode]) -> bool {
    let ids: BTreeSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let by_id: BTreeMap<&str, &TaskNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut visited = BTreeSet::new();
    let mut stack = BTreeSet::new();

    fn dfs<'a>(
        id: &'a str,
        by_id: &BTreeMap<&'a str, &'a TaskNode>,
        known: &BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
        stack: &mut BTreeSet<&'a str>,
    ) -> bool {
        if stack.contains(id) {
            return true;
        }
        if visited.contains(id) {
            return false;
        }
        visited.insert(id);
        stack.insert(id);
        if let Some(node) = by_id.get(id) {
            for dep in &node.depends_on {
                if known.contains(dep.as_str()) && dfs(dep.as_str(), by_id, known, visited, stack) {
                    return true;
                }
            }
        }
        stack.remove(id);
        false
    }

    for id in &ids {
        if dfs(id, &by_id, &ids, &mut visited, &mut stack) {
            return true;
        }
    }
    false
}

fn json_to_sol(j: &serde_json::Value) -> Result<SolValue, SolError> {
    Ok(match j {
        serde_json::Value::Null => SolValue::Null,
        serde_json::Value::Bool(b) => SolValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SolValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                SolValue::float(f)?
            } else {
                return Err(SolError::NonFinite);
            }
        }
        serde_json::Value::String(s) => SolValue::str(s.clone()),
        serde_json::Value::Array(items) => {
            SolValue::List(items.iter().map(json_to_sol).collect::<Result<_, _>>()?)
        }
        serde_json::Value::Object(map) => SolValue::Map(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), json_to_sol(v)?)))
                .collect::<Result<_, SolError>>()?,
        ),
    })
}

fn join_spec_to_sol(spec: &JoinSpec) -> SolValue {
    match spec {
        JoinSpec::LastNode { node_id, field } => SolValue::map([
            ("field", SolValue::str(field.clone())),
            ("kind", SolValue::str("last_node")),
            ("node_id", SolValue::str(node_id.clone())),
        ]),
        JoinSpec::MergeMap { node_fields } => {
            let mut fields = node_fields.clone();
            fields.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            SolValue::map([
                ("kind", SolValue::str("merge_map")),
                (
                    "node_fields",
                    SolValue::List(
                        fields
                            .into_iter()
                            .map(|(node_id, field)| {
                                SolValue::List(vec![
                                    SolValue::str(node_id),
                                    SolValue::str(field),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        }
    }
}

fn slot_ref_to_sol(input: &SlotRef) -> Result<SolValue, SolError> {
    Ok(match input {
        SlotRef::Slot { name } => SolValue::map([
            ("kind", SolValue::str("slot")),
            ("name", SolValue::str(name.clone())),
        ]),
        SlotRef::Literal { value } => SolValue::map([
            ("kind", SolValue::str("literal")),
            ("value", json_to_sol(value)?),
        ]),
        SlotRef::NodeOutput { node_id, field } => SolValue::map([
            ("field", SolValue::str(field.clone())),
            ("kind", SolValue::str("node_output")),
            ("node_id", SolValue::str(node_id.clone())),
        ]),
    })
}

fn slot_specs_to_sol(outputs: &[SlotSpec]) -> SolValue {
    SolValue::List(
        outputs
            .iter()
            .map(|spec| {
                SolValue::map([
                    ("name", SolValue::str(spec.name.clone())),
                    ("ty", SolValue::str(spec.ty.clone())),
                ])
            })
            .collect(),
    )
}

fn slot_refs_to_sol(inputs: &[SlotRef]) -> Result<SolValue, SolError> {
    Ok(SolValue::List(
        inputs.iter().map(slot_ref_to_sol).collect::<Result<_, _>>()?,
    ))
}

fn string_list_to_sol(items: &[String]) -> SolValue {
    SolValue::List(items.iter().map(|item| SolValue::str(item.clone())).collect())
}

fn task_budget_to_sol(budget: &TaskBudget) -> SolValue {
    SolValue::map([
        ("max_ms", SolValue::Int(budget.max_ms as i64)),
        ("max_steps", SolValue::Int(budget.max_steps as i64)),
        ("max_tokens", SolValue::Int(budget.max_tokens as i64)),
    ])
}

fn task_node_to_sol(node: &TaskNode) -> Result<SolValue, SolError> {
    Ok(SolValue::map([
        ("budget", task_budget_to_sol(&node.budget)),
        ("depends_on", string_list_to_sol(&node.depends_on)),
        ("goal", SolValue::str(node.goal.clone())),
        ("id", SolValue::str(node.id.clone())),
        ("inputs", slot_refs_to_sol(&node.inputs)?),
        ("max_refinements", SolValue::Int(node.max_refinements as i64)),
        ("outputs", slot_specs_to_sol(&node.outputs)),
        ("rationale", SolValue::str(node.rationale.clone())),
        ("strategy_hint", string_list_to_sol(&node.strategy_hint)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_linear() -> TaskGraph {
        TaskGraph {
            root_goal: "Calculate average".into(),
            nodes: vec![
                TaskNode {
                    id: "sum".into(),
                    goal: "Sum values".into(),
                    rationale: "Need total".into(),
                    strategy_hint: vec!["std.math.sum".into()],
                    inputs: vec![SlotRef::Slot {
                        name: "values".into(),
                    }],
                    outputs: vec![SlotSpec {
                        name: "total".into(),
                        ty: "float".into(),
                    }],
                    depends_on: vec![],
                    budget: TaskBudget {
                        max_steps: 2,
                        max_tokens: 500,
                        max_ms: 5000,
                    },
                    max_refinements: 1,
                },
                TaskNode {
                    id: "avg".into(),
                    goal: "Divide".into(),
                    rationale: "Complete average".into(),
                    strategy_hint: vec!["std.math.divide".into()],
                    inputs: vec![
                        SlotRef::NodeOutput {
                            node_id: "sum".into(),
                            field: "total".into(),
                        },
                        SlotRef::Slot {
                            name: "count".into(),
                        },
                    ],
                    outputs: vec![SlotSpec {
                        name: "average".into(),
                        ty: "float".into(),
                    }],
                    depends_on: vec!["sum".into()],
                    budget: TaskBudget {
                        max_steps: 2,
                        max_tokens: 500,
                        max_ms: 5000,
                    },
                    max_refinements: 1,
                },
            ],
            join_spec: JoinSpec::LastNode {
                node_id: "avg".into(),
                field: "average".into(),
            },
        }
    }

    #[test]
    fn valid_linear_graph_passes() {
        let graph = sample_linear();
        graph.validate(&TaskBudget::default_turn()).expect("valid");
        let waves = graph.waves().expect("waves");
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0][0].id, "sum");
        assert_eq!(waves[1][0].id, "avg");
    }

    #[test]
    fn cycle_is_rejected() {
        let graph = TaskGraph {
            root_goal: "cycle".into(),
            nodes: vec![
                TaskNode {
                    id: "a".into(),
                    goal: "A".into(),
                    rationale: "r".into(),
                    strategy_hint: vec![],
                    inputs: vec![],
                    outputs: vec![],
                    depends_on: vec!["b".into()],
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
                node_id: "a".into(),
                field: "result".into(),
            },
        };
        assert!(matches!(
            graph.validate(&TaskBudget::default_turn()),
            Err(TaskGraphValidationError::CycleDetected)
        ));
    }

    #[test]
    fn budget_overrun_rejected() {
        let graph = TaskGraph {
            root_goal: "heavy".into(),
            nodes: vec![TaskNode {
                id: "heavy".into(),
                goal: "x".into(),
                rationale: "r".into(),
                strategy_hint: vec![],
                inputs: vec![],
                outputs: vec![],
                depends_on: vec![],
                budget: TaskBudget {
                    max_steps: 100,
                    max_tokens: 50_000,
                    max_ms: 120_000,
                },
                max_refinements: 0,
            }],
            join_spec: JoinSpec::LastNode {
                node_id: "heavy".into(),
                field: "result".into(),
            },
        };
        let parent = TaskBudget {
            max_steps: 10,
            max_tokens: 5000,
            max_ms: 30_000,
        };
        assert!(matches!(
            graph.validate(&parent),
            Err(TaskGraphValidationError::BudgetOverrun { .. })
        ));
    }

    #[test]
    fn parallel_wave_when_no_dependencies() {
        let graph = TaskGraph {
            root_goal: "parallel".into(),
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
                    depends_on: vec![],
                    budget: TaskBudget {
                        max_steps: 1,
                        max_tokens: 100,
                        max_ms: 1000,
                    },
                    max_refinements: 0,
                },
            ],
            join_spec: JoinSpec::LastNode {
                node_id: "a".into(),
                field: "result".into(),
            },
        };
        let waves = graph.waves().expect("waves");
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].len(), 2);
    }

    fn independent_node(id: &str) -> TaskNode {
        TaskNode {
            id: id.into(),
            goal: id.into(),
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
        }
    }

    /// F-032: same-wave node order must be a pure function of the graph, not of a per-process
    /// hash seed — `waves()` used to build `ready` by iterating a `HashSet`, whose order is
    /// randomized per run. Deliberately inserted out of id-sort order to catch a regression back
    /// to Hash{Map,Set}, which would very likely still pass by coincidence within one process.
    #[test]
    fn wave_order_is_deterministic_across_repeated_calls() {
        let graph = TaskGraph {
            root_goal: "parallel-many".into(),
            nodes: vec![
                independent_node("zeta"),
                independent_node("alpha"),
                independent_node("mu"),
                independent_node("beta"),
            ],
            join_spec: JoinSpec::LastNode {
                node_id: "zeta".into(),
                field: "result".into(),
            },
        };
        let first = graph.waves().expect("waves");
        assert_eq!(first.len(), 1);
        let first_ids: Vec<&str> = first[0].iter().map(|n| n.id.as_str()).collect();
        assert_eq!(first_ids, vec!["alpha", "beta", "mu", "zeta"]);

        for _ in 0..20 {
            let waves = graph.waves().expect("waves");
            let ids: Vec<&str> = waves[0].iter().map(|n| n.id.as_str()).collect();
            assert_eq!(ids, first_ids, "wave order must not vary across calls");
        }
    }

    #[test]
    fn canonical_hash_is_stable_across_node_insertion_order() {
        let node_a = independent_node("alpha");
        let node_b = {
            let mut n = independent_node("beta");
            n.depends_on = vec!["alpha".into()];
            n
        };
        let forward = TaskGraph {
            root_goal: "ordered".into(),
            nodes: vec![node_a.clone(), node_b.clone()],
            join_spec: JoinSpec::LastNode {
                node_id: "beta".into(),
                field: "result".into(),
            },
        };
        let reverse = TaskGraph {
            root_goal: "ordered".into(),
            nodes: vec![node_b, node_a],
            join_spec: JoinSpec::LastNode {
                node_id: "beta".into(),
                field: "result".into(),
            },
        };
        assert_eq!(forward.canonical_hash(), reverse.canonical_hash());
        assert_eq!(forward.canonical_bytes(), reverse.canonical_bytes());
    }
}
