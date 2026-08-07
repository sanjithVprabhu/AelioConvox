//! Orchestrate block — Harness v2 cold-path decomposition (F-027).

use crate::abilities::express;
use crate::abilities::learn::SituationKey;
use crate::blocks::tool_call::ToolCallContext;
use crate::blocks::turn::{TurnInput, TurnResult, TurnRuntime, TurnTraceStep};
use crate::orchestration::suspension::{
    GraphSuspension, GraphSuspensionReason, SuspendedExecutorSnapshot,
};
use crate::orchestration::{
    classify_complexity, execute_task_graph, execute_task_graph_from_state,
    observe_task_graph_success, propose_task_graph, GraphExecution, RecursionGuard,
};
use crate::policy::PolicyCtx;
use crate::types::{AelioResult, Depth, LookupTier, Value};
use aelio_sol::{ComplexityClass, TaskBudget};
use chrono::Utc;
use indexmap::IndexMap;

/// Resume a mid-graph suspension after the user supplies missing parameters.
pub fn try_resume_orchestrated_graph(
    rt: &mut TurnRuntime<'_>,
    input: &TurnInput,
    suspension: &GraphSuspension,
    situation_hash: &str,
    sigma: &SituationKey,
    steps: &mut Vec<TurnTraceStep>,
    llm_calls: u32,
) -> Option<TurnResult> {
    steps.push(TurnTraceStep {
        name: "Orchestrate.Resume".into(),
        detail: format!("pending_node={}", suspension.pending_node_id),
    });

    let mut extra_slots = suspension.slots.clone();
    for key in match &suspension.reason {
        GraphSuspensionReason::AwaitingInfo { missing } => missing,
        GraphSuspensionReason::AwaitingConfirmation { .. } => return None,
    } {
        extra_slots
            .entry(key.clone())
            .or_insert_with(|| Value::Str(input.utterance.clone()));
    }
    extra_slots.extend(orchestration_slots(input));

    let policy = PolicyCtx {
        state: Some(input.state_id.clone()),
        tenant: Some(rt.tenant.tenant_id.clone()),
        slots: extra_slots.clone(),
        ..Default::default()
    };
    let mut effect_seq = 0;
    let execution = execute_task_graph_from_state(
        &suspension.graph,
        &input.utterance,
        rt.registry,
        &mut ToolCallContext {
            policies: rt.policies,
            policy: &policy,
            host: rt.tool_host,
            signatures: rt.signatures,
            once_seen: rt.once_seen,
            effects: rt.effect_env,
            user_id: &input.user_id,
            channel: &input.channel,
            turn_key: &input.turn_id,
            effect_seq: &mut effect_seq,
            element_index: None,
        },
        &TaskBudget::default_turn(),
        &extra_slots,
        Some(suspension.executor_state.restore()),
    );

    finish_orchestrated_execution(
        rt,
        input,
        sigma,
        &suspension.graph,
        execution,
        steps,
        llm_calls,
        situation_hash,
        Some(suspension.graph.clone()),
        Some(suspension),
    )
}

/// Attempt orchestrated execution before flat ProposePath. Returns `Some` when handled.
pub fn try_orchestrated_cold_path(
    rt: &mut TurnRuntime<'_>,
    input: &TurnInput,
    clause: &str,
    sigma: &SituationKey,
    situation_hash: &str,
    steps: &mut Vec<TurnTraceStep>,
    llm_calls: u32,
) -> Option<TurnResult> {
    let complexity = classify_complexity(clause);
    steps.push(TurnTraceStep {
        name: "Orchestrate.Classify".into(),
        detail: format!("complexity={complexity:?}"),
    });

    if complexity == ComplexityClass::Atomic {
        return None;
    }

    let extra_slots = orchestration_slots(input);
    let Some(graph) = propose_task_graph(clause, complexity, rt.registry, &extra_slots) else {
        steps.push(TurnTraceStep {
            name: "Orchestrate.Skip".into(),
            detail: "no pinned workflow, std tool, or ephemeral match".into(),
        });
        return None;
    };
    if graph.validate(&TaskBudget::default_turn()).is_err() {
        steps.push(TurnTraceStep {
            name: "Orchestrate.Skip".into(),
            detail: "proposed graph failed budget/shape validation".into(),
        });
        return None;
    }
    let is_ephemeral = graph
        .nodes
        .iter()
        .any(|n| n.strategy_hint.iter().any(|h| h.contains("ephemeral")));
    let route_hint = if is_ephemeral {
        "ephemeral"
    } else if graph.nodes.iter().any(|n| {
        n.strategy_hint.iter().any(|h| {
            rt.registry
                .tools
                .get(h)
                .is_some_and(crate::orchestration::is_client_tool)
        })
    }) {
        "client"
    } else {
        "standard"
    };
    steps.push(TurnTraceStep {
        name: "Orchestrate.Graph".into(),
        detail: format!(
            "nodes={} route={route_hint} hash={}",
            graph.nodes.len(),
            &graph.canonical_hash()[..16.min(graph.canonical_hash().len())]
        ),
    });

    let mut guard = RecursionGuard::new(Some(3));
    if guard.enter().is_err() {
        steps.push(TurnTraceStep {
            name: "Orchestrate.Recursion".into(),
            detail: "recursion depth exceeded".into(),
        });
        return None;
    }

    let policy = PolicyCtx {
        state: Some(input.state_id.clone()),
        tenant: Some(rt.tenant.tenant_id.clone()),
        slots: extra_slots.clone(),
        ..Default::default()
    };
    let mut effect_seq = 0;
    let execution = execute_task_graph(
        &graph,
        clause,
        rt.registry,
        &mut ToolCallContext {
            policies: rt.policies,
            policy: &policy,
            host: rt.tool_host,
            signatures: rt.signatures,
            once_seen: rt.once_seen,
            effects: rt.effect_env,
            user_id: &input.user_id,
            channel: &input.channel,
            turn_key: &input.turn_id,
            effect_seq: &mut effect_seq,
            element_index: None,
        },
        &TaskBudget::default_turn(),
        &extra_slots,
    );

    guard.exit();

    let graph_for_suspension = graph.clone();
    finish_orchestrated_execution(
        rt,
        input,
        sigma,
        &graph,
        execution,
        steps,
        llm_calls,
        situation_hash,
        Some(graph_for_suspension),
        None,
    )
}

fn finish_orchestrated_execution(
    rt: &mut TurnRuntime<'_>,
    input: &TurnInput,
    sigma: &SituationKey,
    graph: &aelio_sol::TaskGraph,
    execution: AelioResult<GraphExecution>,
    steps: &mut Vec<TurnTraceStep>,
    llm_calls: u32,
    situation_hash: &str,
    graph_for_suspension: Option<aelio_sol::TaskGraph>,
    resumed_suspension: Option<&GraphSuspension>,
) -> Option<TurnResult> {
    match execution {
        Ok(GraphExecution::Done(exec)) => {
            if let Some(suspension) = resumed_suspension {
                rt.effect_env.metric_inc("graph_suspension.resumed", 1.0);
                steps.push(TurnTraceStep {
                    name: "Orchestrate.ResumeLinked".into(),
                    detail: format!(
                        "suspended_turn={} resumed_turn={}",
                        suspension.turn_id, input.turn_id
                    ),
                });
            }
            // DurableRuntime persists `TurnResult.steps` as the turn's existing audit seam.
            // Record only hashes: replay data remains available without duplicating node inputs
            // or outputs into the human-readable trace.
            for trace in &exec.trace {
                steps.push(TurnTraceStep {
                    name: "Orchestrate.Trace".into(),
                    detail: format!(
                        "seq={} inputs_hash={} output_hash={}",
                        trace.seq,
                        hex::encode(trace.inputs_hash),
                        hex::encode(trace.output_hash)
                    ),
                });
            }
            steps.push(TurnTraceStep {
                name: "Orchestrate.Execute".into(),
                detail: format!(
                    "nodes={} ledger={} trace={} refinements={} ephemeral_created={} ephemeral_destroyed={}",
                    exec.nodes_executed,
                    exec.ledger.len(),
                    exec.trace.len(),
                    exec.refinements_used,
                    exec.ephemeral_created,
                    exec.ephemeral_destroyed
                ),
            });

            let evidence = format_result_evidence(&exec.joined);
            let reply = express::synthesize(&evidence, rt.personality, &[], None).0;

            let outcome = observe_task_graph_success(
                rt.proposals,
                rt.registry,
                sigma,
                graph,
                true,
                false,
                rt.embedder,
            );
            steps.push(TurnTraceStep {
                name: "Orchestrate.Promote".into(),
                detail: format!("{outcome:?}"),
            });

            Some(TurnResult {
                reply,
                llm_calls,
                tier: Some(LookupTier::Tier3),
                depth: Depth::Deep,
                steps: std::mem::take(steps),
                opened_loop: false,
                new_state: None,
                active_flow: input.active_flow.clone(),
                situation_hash: Some(situation_hash.to_string()),
                proposal_id: None,
                suspended: false,
                graph_suspension: None,
            })
        }
        Ok(GraphExecution::NeedUser {
            missing,
            question,
            pending_node_id,
            executor_state,
            slots,
        }) => {
            steps.push(TurnTraceStep {
                name: "Orchestrate.NeedUser".into(),
                detail: format!("unbound required params: {}", missing.join(",")),
            });
            let mut slots = slots;
            // Invalid user input must never become a durable slot. The next answer gets one fresh
            // binding attempt while valid values for other parameters remain available.
            for key in &missing {
                slots.shift_remove(key);
            }
            let suspension = graph_for_suspension.map(|graph| {
                let now_ms = Utc::now().timestamp_millis();
                let mut next = GraphSuspension::awaiting_info(
                    input.turn_id.clone(),
                    input.user_id.clone(),
                    graph,
                    slots,
                    SuspendedExecutorSnapshot::from(&executor_state),
                    pending_node_id,
                    missing.clone(),
                    now_ms,
                );
                if let Some(previous) = resumed_suspension {
                    next.turn_id = previous.turn_id.clone();
                    next.retry_count = previous.retry_count.saturating_add(1);
                    next.max_retries = previous.max_retries;
                    next.opened_at_ms = previous.opened_at_ms;
                    next.expires_at_ms = previous.expires_at_ms;
                } else {
                    rt.effect_env.metric_inc("graph_suspension.opened", 1.0);
                }
                next
            });
            if suspension.as_ref().is_some_and(|next| !next.can_retry()) {
                rt.effect_env.metric_inc("graph_suspension.abandoned", 1.0);
                steps.push(TurnTraceStep {
                    name: "Orchestrate.Abandoned".into(),
                    detail: "clarification retry limit reached; next turn starts fresh".into(),
                });
                return Some(TurnResult {
                    reply: express::synthesize(
                        "I could not validate that information. Please start the request again.",
                        None,
                        &[],
                        None,
                    )
                    .0,
                    llm_calls,
                    tier: Some(LookupTier::Tier3),
                    depth: Depth::Deep,
                    steps: std::mem::take(steps),
                    opened_loop: false,
                    new_state: None,
                    active_flow: input.active_flow.clone(),
                    situation_hash: Some(situation_hash.to_string()),
                    proposal_id: None,
                    suspended: false,
                    graph_suspension: None,
                });
            }
            Some(need_user_turn_result(
                question,
                situation_hash,
                steps,
                llm_calls,
                input,
                suspension,
            ))
        }
        Err(error) => {
            steps.push(TurnTraceStep {
                name: "Orchestrate.Skip".into(),
                detail: format!("execution failed: {:?}: {}", error.code, error.message),
            });
            None
        }
    }
}

fn need_user_turn_result(
    question: String,
    situation_hash: &str,
    steps: &mut Vec<TurnTraceStep>,
    llm_calls: u32,
    input: &TurnInput,
    graph_suspension: Option<GraphSuspension>,
) -> TurnResult {
    TurnResult {
        reply: express::synthesize(&question, None, &[], None).0,
        llm_calls,
        tier: Some(LookupTier::Tier3),
        depth: Depth::Deep,
        steps: std::mem::take(steps),
        opened_loop: true,
        new_state: None,
        active_flow: input.active_flow.clone(),
        situation_hash: Some(situation_hash.to_string()),
        proposal_id: None,
        suspended: true,
        graph_suspension,
    }
}

fn orchestration_slots(input: &TurnInput) -> IndexMap<String, Value> {
    input
        .slots
        .iter()
        .map(|(name, value)| (name.clone(), crate::ops::pure::json_to_value(value)))
        .collect()
}

fn format_result_evidence(value: &Value) -> String {
    match value {
        Value::Int(i) => format!("The computed result is {i}."),
        Value::Float(f) => format!("The computed result is {f}."),
        Value::Str(s) => format!("The computed result is {s}."),
        Value::Map(map) => {
            let parts: Vec<String> = map.iter().map(|(k, v)| format!("{k}={v:?}")).collect();
            format!("The computed results are: {}.", parts.join(", "))
        }
        other => format!("The computed result is {other:?}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::World;
    use crate::types::Depth;

    #[test]
    fn need_user_returns_suspended_turn() {
        let mut steps = Vec::new();
        let input = TurnInput {
            turn_id: "t1".into(),
            user_id: "u1".into(),
            channel: "web".into(),
            utterance: "notify me".into(),
            state_id: "default".into(),
            slots: IndexMap::new(),
            turn_index: 1,
            last_seen_secs_ago: None,
            active_flow: None,
            graph_suspension: None,
        };
        let result = need_user_turn_result(
            "What phone number should I use?".into(),
            "abc123",
            &mut steps,
            0,
            &input,
            None,
        );
        assert!(result.suspended);
        assert!(result.opened_loop);
        assert_eq!(result.depth, Depth::Deep);
        assert!(!result.reply.text.is_empty());
    }

    #[test]
    fn orchestrated_average_turn_via_world() {
        let mut world = World::demo_tenant("test");
        // Force cold path: utterance that Conductor escalates and orchestrator handles.
        let result = world.run_turn("u1", "calculate the average of 10 20 30");
        assert!(
            result.steps.iter().any(|s| s.name == "Orchestrate.Execute"),
            "expected orchestrated execution, steps={:?}",
            result.steps
        );
    }
}
