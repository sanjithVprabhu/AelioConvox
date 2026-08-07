//! Promotion integration — store successful TaskGraphs for warm-path reuse.

use crate::abilities::learn::{observe_and_promote, path_is_effectful, ObserveOutcome, SituationKey};
use crate::abilities::registry::Registry;
use crate::contract::{AbilityPath, PathStep};
use crate::types::{AelioError, AelioResult, ReasonCode};
use crate::embedding::Embedder;
use aelio_sol::TaskGraph;
use indexmap::IndexMap;
use std::collections::{BTreeSet, VecDeque};

/// Observe a successful orchestrated graph and feed the learning gate.
pub fn observe_task_graph_success(
    proposals: &mut crate::abilities::learn::ProposalMap,
    registry: &mut Registry,
    sigma: &SituationKey,
    graph: &TaskGraph,
    success: bool,
    tenant_approved: bool,
    embedder: &dyn Embedder,
) -> ObserveOutcome {
    if let Err(error) = reject_incomplete_aggregate_graph(registry, graph) {
        return ObserveOutcome::Blocked {
            reason: error.message,
        };
    }
    let path = task_graph_to_ability_path(graph);
    let effectful = path_is_effectful(registry, &path);
    observe_and_promote(
        proposals,
        registry,
        sigma,
        path,
        effectful,
        success,
        tenant_approved,
        embedder,
    )
}

/// Refuse promotion when an aggregate consumes a source whose result set is not declared complete.
/// `Unknown` is a valid operational value, but it cannot be used to teach a workflow that claims
/// an aggregate answer. The graph is walked through all dependency ancestors, not only direct
/// edges, so filter/map nodes cannot launder an incomplete source into `mean`.
pub fn reject_incomplete_aggregate_graph(registry: &Registry, graph: &TaskGraph) -> AelioResult<()> {
    let aggregate_ids = [
        "std.math.average",
        "std.math.sum",
        "std.math.min",
        "std.math.max",
    ];
    let nodes = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<std::collections::BTreeMap<_, _>>();

    for aggregate in graph.nodes.iter().filter(|node| {
        node.strategy_hint
            .iter()
            .any(|tool_id| aggregate_ids.contains(&tool_id.as_str()))
    }) {
        let mut pending = VecDeque::from(aggregate.depends_on.clone());
        let mut visited = BTreeSet::new();
        while let Some(node_id) = pending.pop_front() {
            if !visited.insert(node_id.clone()) {
                continue;
            }
            let node = nodes.get(node_id.as_str()).ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Validation,
                    format!("aggregate `{}` depends on unknown node `{node_id}`", aggregate.id),
                )
            })?;
            for tool_id in &node.strategy_hint {
                if let Some(tool) = registry.tools.get(tool_id) {
                    let contract = tool.contract.as_ref().ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::Validation,
                            format!("tool `{tool_id}` has no ToolContract"),
                        )
                    })?;
                    if contract.completeness.blocks_aggregate_promotion() {
                        crate::abilities::registry::record_promotion_completeness_block();
                        return Err(AelioError::new(
                            ReasonCode::Validation,
                            format!(
                                "promotion refused: aggregate `{}` is downstream of incomplete tool `{tool_id}` ({:?})",
                                aggregate.id, contract.completeness
                            ),
                        ));
                    }
                }
            }
            pending.extend(node.depends_on.iter().cloned());
        }
    }
    Ok(())
}

fn task_graph_to_ability_path(graph: &TaskGraph) -> AbilityPath {
    let steps = graph
        .nodes
        .iter()
        .map(|node| PathStep {
            ability_id: node
                .strategy_hint
                .first()
                .cloned()
                .unwrap_or_else(|| "Orchestration.Node".into()),
            args: IndexMap::from([(
                "goal".into(),
                serde_json::Value::String(node.goal.clone()),
            )]),
        })
        .collect();
    AbilityPath { steps }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abilities::registry::Registry;
    use crate::orchestration::orchestrate::propose_task_graph;
    use crate::orchestration::register_standard_tools;
    use aelio_sol::{ComplexityClass, JoinSpec, TaskBudget, TaskNode};
    use indexmap::IndexMap;

    #[test]
    fn graph_converts_to_ability_path() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        let graph = propose_task_graph(
            "average of 1 2 3",
            ComplexityClass::Composite,
            &registry,
            &IndexMap::new(),
        )
        .unwrap();
        let path = task_graph_to_ability_path(&graph);
        assert_eq!(path.steps.len(), graph.nodes.len());
    }

    #[test]
    fn unknown_source_refuses_aggregate_promotion_and_names_tool() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry
            .register_tool(crate::tenant::ToolSpec {
                id: "people_search".into(),
                name: "people_search".into(),
                version: "1".into(),
                capability_tags: vec!["people.search".into()],
                contract: Some(crate::tenant::ToolContract {
                    effect_class: crate::tenant::EffectClass::Read,
                    completeness: crate::tenant::Completeness::Unknown,
                    returns_entity: "person".into(),
                    pushdown: vec!["filter".into()],
                    max_result_rows: None,
                    row_scoped: true,
                }),
                effect: Some(crate::tenant::ToolEffect::Read),
                effectful: false,
                idempotent: true,
                dry_run_available: true,
                params: vec![],
                output_semantics: crate::tenant::OutputSpec {
                    fields: IndexMap::new(),
                    role_hint: None,
                },
                continuations: vec![],
                errors: vec![],
            })
            .unwrap();
        let budget = TaskBudget::default_turn();
        let graph = TaskGraph {
            root_goal: "mean filtered people".into(),
            nodes: vec![
                TaskNode {
                    id: "source".into(),
                    goal: "load people".into(),
                    rationale: "source".into(),
                    strategy_hint: vec!["people_search".into()],
                    inputs: vec![],
                    outputs: vec![],
                    depends_on: vec![],
                    budget: budget.clone(),
                    max_refinements: 0,
                },
                TaskNode {
                    id: "filter".into(),
                    goal: "filter people".into(),
                    rationale: "filter".into(),
                    strategy_hint: vec!["std.data.filter_equals".into()],
                    inputs: vec![],
                    outputs: vec![],
                    depends_on: vec!["source".into()],
                    budget: budget.clone(),
                    max_refinements: 0,
                },
                TaskNode {
                    id: "mean".into(),
                    goal: "mean age".into(),
                    rationale: "aggregate".into(),
                    strategy_hint: vec!["std.math.average".into()],
                    inputs: vec![],
                    outputs: vec![],
                    depends_on: vec!["filter".into()],
                    budget,
                    max_refinements: 0,
                },
            ],
            join_spec: JoinSpec::LastNode {
                node_id: "mean".into(),
                field: "average".into(),
            },
        };
        let error = reject_incomplete_aggregate_graph(&registry, &graph)
            .expect_err("Unknown source must block aggregate promotion");
        assert!(error.message.contains("people_search"));
        assert!(
            crate::abilities::registry::contract_metrics()
                .promotion_blocked_completeness_total
                >= 1
        );
    }
}
