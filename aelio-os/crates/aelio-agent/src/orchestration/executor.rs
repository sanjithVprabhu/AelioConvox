//! TaskGraph execution — wavefront over nodes with std tool dispatch.
//!
//! Routing invariant: `std.*` and ephemeral nodes execute locally in-process; a node naming a
//! tenant-registered tool goes through [`run_tool_call_block`], which applies policy, parameter
//! binding, a derived idempotency key, signature capture, and response redaction. The executor
//! never calls a tenant host directly.

use crate::abilities::bind::BindSources;
use crate::abilities::registry::Registry;
use crate::blocks::tool_call::{run_tool_call_block, ToolCallContext, ToolCallOutcome};
use crate::orchestration::ephemeral::{adapt_with_scope, EphemeralScope};
use crate::orchestration::evaluate::{evaluate_shape, join_outputs, should_refine};
use crate::orchestration::pins::extract_numbers;
use crate::orchestration::stdlib::{invoke_standard_tool, is_standard_tool};
use crate::orchestration::tool_router::{
    resolve_tool_for_node, route_for_tool, tool_catalog_for_context, ToolRoute,
};
use crate::orchestration::wavefront::{
    invoke_node_tool, next_wave, resolve_node_inputs, ExecutorState, LedgerEntry, NodeInvocation,
};
use crate::policy::{require_allow, PolicyActionRisk};
use crate::tenant::{ParamSource, ToolSpec};
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use aelio_sol::{TaskBudget, TaskGraph, TaskNode};
use harness_core::exec::{hash_step_payload, Step};
use indexmap::IndexMap;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct GraphExecutionResult {
    pub joined: Value,
    /// Per-node execution record. Deterministic for a given input: `args_hash` is BLAKE3 over
    /// canonical args and `result` carries no clock- or identity-derived values.
    pub ledger: Vec<LedgerEntry>,
    /// HKv4 replay trace for the completed graph. `seq` is the node's planned topological index,
    /// never its wall-clock completion position.
    pub trace: Vec<Step>,
    pub nodes_executed: usize,
    pub refinements_used: u32,
    pub ephemeral_created: usize,
    pub ephemeral_destroyed: usize,
}

/// Result of running a whole graph. `NeedUser` surfaces a client tool's unbound required
/// parameter so the turn can ask, rather than invoking with a missing argument.
#[derive(Debug, Clone)]
pub enum GraphExecution {
    Done(GraphExecutionResult),
    NeedUser {
        missing: Vec<String>,
        question: String,
        pending_node_id: String,
        executor_state: crate::orchestration::wavefront::ExecutorState,
        slots: IndexMap<String, Value>,
    },
}

enum NodeStep {
    Done,
    NeedUser {
        missing: Vec<String>,
        question: String,
    },
}

/// Execute a validated TaskGraph using standard tools and registry bindings.
pub fn execute_task_graph(
    graph: &TaskGraph,
    utterance: &str,
    registry: &Registry,
    tools: &mut ToolCallContext<'_>,
    parent_budget: &TaskBudget,
    extra_slots: &IndexMap<String, Value>,
) -> AelioResult<GraphExecution> {
    execute_task_graph_from_state(
        graph,
        utterance,
        registry,
        tools,
        parent_budget,
        extra_slots,
        None,
    )
}

/// Resume or start graph execution from an optional partial executor snapshot.
pub fn execute_task_graph_from_state(
    graph: &TaskGraph,
    utterance: &str,
    registry: &Registry,
    tools: &mut ToolCallContext<'_>,
    parent_budget: &TaskBudget,
    extra_slots: &IndexMap<String, Value>,
    initial_state: Option<crate::orchestration::wavefront::ExecutorState>,
) -> AelioResult<GraphExecution> {
    graph
        .validate(parent_budget)
        .map_err(|e| AelioError::new(ReasonCode::Validation, e.to_string()))?;

    let slots = build_initial_slots(utterance, extra_slots, registry);
    let mut state = initial_state.unwrap_or_default();
    let mut refinements_used = 0u32;
    let mut ephemeral_scope = EphemeralScope::default();

    loop {
        let wave = next_wave(graph, &state.completed)?;
        if wave.is_empty() {
            break;
        }
        for node in wave {
            match execute_one_node(
                node,
                registry,
                tools,
                utterance,
                &slots,
                &mut state,
                &mut refinements_used,
                &mut ephemeral_scope,
            )? {
                NodeStep::Done => {}
                NodeStep::NeedUser { missing, question } => {
                    return Ok(GraphExecution::NeedUser {
                        missing,
                        question,
                        pending_node_id: node.id.clone(),
                        executor_state: state.clone(),
                        slots: slots.clone(),
                    });
                }
            }
        }
    }

    let mut outputs = IndexMap::new();
    for (node_id, value) in &state.outputs {
        outputs.insert(node_id.clone(), value.clone());
    }
    let joined = join_outputs(&graph.join_spec, &outputs)?;
    let trace = trace_from_ledger(graph, &state.ledger)?;

    Ok(GraphExecution::Done(GraphExecutionResult {
        joined,
        ledger: state.ledger,
        trace,
        nodes_executed: state.completed.len(),
        refinements_used,
        ephemeral_created: ephemeral_scope.created.len(),
        ephemeral_destroyed: ephemeral_scope.destroyed.len(),
    }))
}

/// Build HKv4 steps from completed live invocations, preserving the graph's planned topology.
///
/// The executor is currently sequential within each wave, but trace ordering must remain stable
/// when wave execution becomes concurrent. The plan, not completion timing, is the authority.
fn trace_from_ledger(graph: &TaskGraph, ledger: &[LedgerEntry]) -> AelioResult<Vec<Step>> {
    let planned_indices: BTreeMap<&str, u64> = graph
        .waves()
        .map_err(|error| AelioError::new(ReasonCode::Validation, error.to_string()))?
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), index as u64))
        .collect();

    let mut trace = Vec::with_capacity(ledger.len());
    for entry in ledger {
        let seq = planned_indices
            .get(entry.node_id.as_str())
            .copied()
            .ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Internal,
                    format!(
                        "ledger entry references node absent from graph: `{}`",
                        entry.node_id
                    ),
                )
            })?;
        trace.push(Step {
            seq,
            inputs_hash: hash_step_payload(
                "aelio-agent.task-graph.inputs.v1",
                entry.args_hash.as_bytes(),
            ),
            output_hash: hash_value_for_trace(&entry.result)?,
        });
    }
    trace.sort_by_key(|step| step.seq);
    Ok(trace)
}

fn hash_value_for_trace(value: &Value) -> AelioResult<harness_core::exec::ContentHash> {
    let sol = crate::ops::pure::value_to_sol(value)
        .map_err(|error| AelioError::new(ReasonCode::Validation, error.to_string()))?;
    Ok(hash_step_payload(
        "aelio-agent.task-graph.output.v1",
        &aelio_sol::canonical_bytes(&sol),
    ))
}

fn build_initial_slots(
    utterance: &str,
    extra: &IndexMap<String, Value>,
    registry: &Registry,
) -> IndexMap<String, Value> {
    let numbers = extract_numbers(utterance);
    let values: Vec<Value> = numbers
        .into_iter()
        .map(|n| {
            if n.fract() == 0.0 {
                Value::Int(n as i64)
            } else {
                Value::Float(n)
            }
        })
        .collect();
    let count = values.len() as i64;
    let mut slots = IndexMap::new();
    slots.insert("values".into(), Value::List(values.clone()));
    slots.insert("count".into(), Value::Int(count));
    slots.insert("utterance".into(), Value::Str(utterance.into()));
    slots.insert("goal".into(), Value::Str(utterance.into()));

    let mut context = IndexMap::new();
    context.insert("utterance".into(), Value::Str(utterance.into()));
    // Ephemeral pipelines read their inputs from `context`, not the slot namespace, so numeric
    // operands have to be mirrored here or synthesized math steps run over an empty list.
    context.insert("values".into(), Value::List(values));
    context.insert("count".into(), Value::Int(count));
    for key in ["document_text", "document", "attached_text", "text"] {
        if let Some(v) = extra.get(key) {
            context.insert(key.into(), v.clone());
        }
    }
    // Include full tool catalog so ephemeral synthesizer knows what NOT to duplicate.
    for (k, v) in tool_catalog_for_context(registry) {
        context.insert(k, v);
    }
    slots.insert("context".into(), Value::Map(context));

    for (k, v) in extra {
        slots.entry(k.clone()).or_insert_with(|| v.clone());
    }
    slots
}

#[allow(clippy::too_many_arguments)]
fn execute_one_node(
    node: &TaskNode,
    registry: &Registry,
    tools: &mut ToolCallContext<'_>,
    utterance: &str,
    slots: &IndexMap<String, Value>,
    state: &mut ExecutorState,
    refinements_used: &mut u32,
    ephemeral_scope: &mut EphemeralScope,
) -> AelioResult<NodeStep> {
    let tool = resolve_tool_for_node(node, registry)
        .map_err(|e| AelioError::new(ReasonCode::NotFound, e))?;
    let route = route_for_tool(&tool);
    let resolved = resolve_node_inputs(node, slots, &state.outputs)?;

    let mut attempt = 0u8;
    loop {
        let invocation = match route {
            ToolRoute::Client => {
                invoke_client_node(node, &tool, tools, slots, &resolved, utterance, state)?
            }
            ToolRoute::Standard | ToolRoute::Ephemeral => {
                let args = map_args_for_tool(&tool, &resolved)?;
                invoke_node_tool(state, node, &tool, args, |t, a| {
                    invoke_local_tool(t, a, tools, ephemeral_scope)
                })?
            }
        };

        let output = match invocation {
            NodeInvocation::Value(value) => value,
            NodeInvocation::NeedUser { missing, question } => {
                return Ok(NodeStep::NeedUser { missing, question });
            }
        };

        let verdict = evaluate_shape(&output, &node.outputs);

        if verdict.pass {
            return Ok(NodeStep::Done);
        }
        if should_refine(node, attempt, &verdict) {
            attempt += 1;
            *refinements_used += 1;
            continue;
        }
        return Err(AelioError::new(
            ReasonCode::Validation,
            format!(
                "node `{}` evaluation failed after {} refinement(s): {}",
                node.id, attempt, verdict.repair_hint
            ),
        ));
    }
}

/// Invoke a tenant-registered tool through the full gate. Parameters are bound from the turn
/// slots by the tool's own declared `ParamSource`s — orchestration never hands a tenant tool an
/// untyped `context` blob.
fn invoke_client_node(
    node: &TaskNode,
    tool: &ToolSpec,
    tools: &mut ToolCallContext<'_>,
    slots: &IndexMap<String, Value>,
    resolved: &IndexMap<String, Value>,
    utterance: &str,
    state: &mut ExecutorState,
) -> AelioResult<NodeInvocation> {
    let mut bind_slots = slots.clone();
    for (name, value) in resolved {
        bind_slots.insert(name.clone(), value.clone());
    }
    let sources = BindSources {
        slots: bind_slots,
        state: IndexMap::new(),
        env: tools.policy.env.clone(),
        tool_outputs: state
            .outputs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        user_text: Some(utterance.to_string()),
    };

    let mut policy = tools.policy.clone();
    policy.tool = Some(tool.id.clone());
    policy.capability = tool.capability_tags.first().cloned();

    // Ledger bookkeeping keys off the node's resolved inputs; `run_tool_call_block` derives its
    // own per-call idempotency key from user, tool, version, and bound arguments.
    invoke_node_tool(state, node, tool, resolved.clone(), |t, _| {
        let mut call = ToolCallContext {
            policies: tools.policies,
            policy: &policy,
            host: &mut *tools.host,
            signatures: &mut *tools.signatures,
            once_seen: &mut *tools.once_seen,
            effects: &mut *tools.effects,
            user_id: tools.user_id,
            channel: tools.channel,
            turn_key: tools.turn_key,
            effect_seq: &mut *tools.effect_seq,
            element_index: tools.element_index,
        };
        match run_tool_call_block(t, &sources, &mut call) {
            Err(error)
                if matches!(
                    error.code,
                    ReasonCode::FormatViolation
                        | ReasonCode::PatternViolation
                        | ReasonCode::RangeViolation
                        | ReasonCode::LengthViolation
                        | ReasonCode::EnumViolation
                        | ReasonCode::TypeViolation
                ) =>
            {
                let invalid = tool.params.iter().find(|param| {
                    param.required
                        && matches!(param.source, ParamSource::User)
                        && param.constraint.is_some()
                });
                if let Some(param) = invalid {
                    Ok(NodeInvocation::NeedUser {
                        missing: vec![param.name.clone()],
                        question: param
                            .prompt_hint
                            .clone()
                            .unwrap_or_else(|| format!("Please provide a valid {}.", param.name)),
                    })
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
            Ok(ToolCallOutcome::NeedUser { missing, question }) => {
                Ok(NodeInvocation::NeedUser { missing, question })
            }
            Ok(ToolCallOutcome::Done { receipt }) => {
                Ok(NodeInvocation::Value(receipt.safe_response))
            }
        }
    })
}

fn map_args_for_tool(
    tool: &ToolSpec,
    resolved: &IndexMap<String, Value>,
) -> AelioResult<IndexMap<String, Value>> {
    let mut args = IndexMap::new();
    if tool.id == "std.math.sum" || tool.id == "std.math.average" {
        args.insert(
            "values".into(),
            resolved
                .get("values")
                .cloned()
                .ok_or_else(|| AelioError::new(ReasonCode::Missing, "values required"))?,
        );
        return Ok(args);
    }
    if tool.id == "std.math.divide" {
        args.insert(
            "a".into(),
            resolved
                .get("total")
                .or_else(|| resolved.get("a"))
                .cloned()
                .ok_or_else(|| AelioError::new(ReasonCode::Missing, "dividend required"))?,
        );
        args.insert(
            "b".into(),
            resolved
                .get("count")
                .or_else(|| resolved.get("b"))
                .cloned()
                .ok_or_else(|| AelioError::new(ReasonCode::Missing, "divisor required"))?,
        );
        return Ok(args);
    }
    if tool.id == "std.ephemeral.adapt" {
        args.insert(
            "goal".into(),
            resolved
                .get("goal")
                .cloned()
                .ok_or_else(|| AelioError::new(ReasonCode::Missing, "goal required"))?,
        );
        args.insert(
            "context".into(),
            resolved
                .get("context")
                .cloned()
                .unwrap_or(Value::Map(IndexMap::new())),
        );
        return Ok(args);
    }
    if tool.id == "std.ephemeral.read_document" {
        args.insert(
            "document".into(),
            resolved
                .get("document")
                .or_else(|| resolved.get("document_text"))
                .cloned()
                .ok_or_else(|| AelioError::new(ReasonCode::Missing, "document required"))?,
        );
        args.insert(
            "query".into(),
            resolved
                .get("query")
                .or_else(|| resolved.get("goal"))
                .cloned()
                .ok_or_else(|| AelioError::new(ReasonCode::Missing, "query required"))?,
        );
        return Ok(args);
    }
    for (k, v) in resolved {
        args.insert(k.clone(), v.clone());
    }
    Ok(args)
}

/// Execute a first-party `std.*` or ephemeral tool in-process, behind the tenant's policy.
fn invoke_local_tool(
    tool: &ToolSpec,
    args: &IndexMap<String, Value>,
    tools: &mut ToolCallContext<'_>,
    ephemeral_scope: &mut EphemeralScope,
) -> AelioResult<NodeInvocation> {
    // std tools are pure and local, but a tenant may still deny specific ones by capability.
    let mut policy = tools.policy.clone();
    policy.tool = Some(tool.id.clone());
    policy.capability = tool.capability_tags.first().cloned();
    policy.action_risk = PolicyActionRisk::ReadOnly;
    require_allow(tools.policies, &policy)?;

    if tool.id == "std.ephemeral.adapt" {
        let goal = args
            .get("goal")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AelioError::new(ReasonCode::Missing, "goal required"))?;
        let context = args
            .get("context")
            .cloned()
            .unwrap_or(Value::Map(IndexMap::new()));
        let (result, _pipeline_id) = adapt_with_scope(ephemeral_scope, goal, &context)?;
        let mut map = IndexMap::new();
        map.insert("result".into(), result);
        return Ok(NodeInvocation::Value(Value::Map(map)));
    }
    if tool.id == "std.ephemeral.read_document" {
        let document = args
            .get("document")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AelioError::new(ReasonCode::Missing, "document required"))?;
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AelioError::new(ReasonCode::Missing, "query required"))?;
        let mut ctx = IndexMap::new();
        ctx.insert("document_text".into(), Value::Str(document.to_string()));
        let (result, _pipeline_id) = adapt_with_scope(ephemeral_scope, query, &Value::Map(ctx))?;
        let mut map = IndexMap::new();
        map.insert("result".into(), result);
        return Ok(NodeInvocation::Value(Value::Map(map)));
    }
    if is_standard_tool(&tool.id) {
        if let Some(result) = invoke_standard_tool(&tool.id, args) {
            return result.map(NodeInvocation::Value);
        }
    }
    Err(AelioError::new(
        ReasonCode::NotFound,
        format!("no local implementation for `{}`", tool.id),
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::abilities::invoke::{CapabilityHost, MockToolHost};
    use crate::abilities::registry::Registry;
    use crate::abilities::sig::SignatureRegistry;
    use crate::ops::effects::EffectEnv;
    use crate::orchestration::orchestrate::propose_task_graph;
    use crate::orchestration::stdlib::register_standard_tools;
    use crate::policy::PolicyCtx;
    use crate::tenant::{
        OutputField, OutputSpec, ParamSource, ParamSpec, PolicySpec, ToolEffect, ToolSpec,
    };
    use crate::types::Sensitivity;
    use aelio_sol::ComplexityClass;
    use std::collections::HashSet;

    /// Owns the borrows a `ToolCallContext` needs so tests can build one in a single statement.
    #[derive(Default)]
    pub(crate) struct TestGate {
        pub policies: Vec<PolicySpec>,
        pub policy: PolicyCtx,
        pub signatures: SignatureRegistry,
        pub once_seen: HashSet<String>,
        pub effects: EffectEnv,
        pub effect_seq: u64,
    }

    impl TestGate {
        pub(crate) fn context<'a>(
            &'a mut self,
            host: &'a mut dyn CapabilityHost,
        ) -> ToolCallContext<'a> {
            ToolCallContext {
                policies: &self.policies,
                policy: &self.policy,
                host,
                signatures: &mut self.signatures,
                once_seen: &mut self.once_seen,
                effects: &mut self.effects,
                user_id: "u1",
                channel: "web",
                turn_key: "test-turn",
                effect_seq: &mut self.effect_seq,
                element_index: None,
            }
        }
    }

    pub(crate) fn done(execution: GraphExecution) -> GraphExecutionResult {
        match execution {
            GraphExecution::Done(result) => result,
            GraphExecution::NeedUser { missing, .. } => {
                panic!("expected completion, needed user input for {missing:?}")
            }
        }
    }

    /// Records the idempotency key the gate derives for every call, so tests can prove distinct
    /// calls are not deduped against one another.
    #[derive(Default)]
    pub(crate) struct RecordingHost {
        pub calls: Vec<(String, IndexMap<String, Value>, String)>,
    }

    impl CapabilityHost for RecordingHost {
        fn call_with_context(
            &mut self,
            tool: &ToolSpec,
            args: &IndexMap<String, Value>,
            idempotency_key: &str,
            _user_id: &str,
            _channel: &str,
        ) -> AelioResult<Value> {
            self.calls
                .push((tool.id.clone(), args.clone(), idempotency_key.to_string()));
            Ok(Value::Map(IndexMap::from([(
                "rows".into(),
                Value::List(vec![]),
            )])))
        }

        fn invocation_count(&self) -> Option<usize> {
            Some(self.calls.len())
        }

        fn invocation_count_for(&self, tool_id: &str) -> Option<usize> {
            Some(self.calls.iter().filter(|(id, _, _)| id == tool_id).count())
        }
    }

    pub(crate) fn deny_policy(id: &str, capability: &str) -> PolicySpec {
        PolicySpec {
            id: id.into(),
            effect: crate::tenant::PolicyEffect::Deny,
            subject: crate::tenant::PolicySubject::default(),
            action: crate::tenant::PolicyAction {
                capability: Some(capability.into()),
                ..Default::default()
            },
            condition: crate::contract::Predicate::True,
            reason_code: "denied_by_tenant".into(),
            priority: 10,
        }
    }

    pub(crate) fn read_tool(id: &str, params: Vec<ParamSpec>) -> ToolSpec {
        ToolSpec {
            id: id.into(),
            name: id.into(),
            version: "1".into(),
            capability_tags: vec![id.into()],
            contract: Some(crate::tenant::ToolContract::complete_read("query_row")),
            effect: Some(ToolEffect::Read),
            effectful: false,
            idempotent: true,
            dry_run_available: true,
            params,
            output_semantics: OutputSpec {
                fields: IndexMap::from([(
                    "rows".into(),
                    OutputField {
                        path: "rows".into(),
                        type_name: "list".into(),
                        sensitivity: Sensitivity::None,
                        meaning: "rows".into(),
                    },
                )]),
                role_hint: None,
            },
            continuations: vec![],
            errors: vec![],
        }
    }

    pub(crate) fn required_slot_param(name: &str) -> ParamSpec {
        ParamSpec {
            name: name.into(),
            type_name: "string".into(),
            required: true,
            constraint: None,
            source: ParamSource::Slot { name: name.into() },
            repair: None,
            prompt_hint: None,
            sensitivity: Sensitivity::None,
            default: None,
            depends_on: vec![],
        }
    }

    #[test]
    fn average_graph_executes_end_to_end() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        let graph = propose_task_graph(
            "calculate the average of 10 20 30",
            ComplexityClass::Composite,
            &registry,
            &IndexMap::new(),
        )
        .expect("graph");
        let mut host = MockToolHost::default();
        let mut gate = TestGate::default();
        let result = done(
            execute_task_graph(
                &graph,
                "calculate the average of 10 20 30",
                &registry,
                &mut gate.context(&mut host),
                &TaskBudget::default_turn(),
                &IndexMap::new(),
            )
            .expect("execute"),
        );
        assert_eq!(result.nodes_executed, 2);
        match result.joined {
            Value::Int(20) => {}
            Value::Float(f) if (f - 20.0).abs() < f64::EPSILON => {}
            other => panic!("unexpected average: {other:?}"),
        }
    }

    #[test]
    fn live_trace_verifies_and_reports_corrupted_middle_output_sequence() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        let graph = propose_task_graph(
            "calculate the average of 10 20 30",
            ComplexityClass::Composite,
            &registry,
            &IndexMap::new(),
        )
        .expect("graph");
        let mut host = MockToolHost::default();
        let mut gate = TestGate::default();
        let result = done(
            execute_task_graph(
                &graph,
                "calculate the average of 10 20 30",
                &registry,
                &mut gate.context(&mut host),
                &TaskBudget::default_turn(),
                &IndexMap::new(),
            )
            .expect("execute"),
        );

        assert_eq!(
            result.trace.iter().map(|step| step.seq).collect::<Vec<_>>(),
            vec![0, 1],
            "sequences must be assigned from the planned graph topology"
        );
        assert!(harness_core::exec::verify_replay(&result.trace, &result.trace).matched);

        let mut corrupted = result.trace.clone();
        corrupted[1].output_hash[0] ^= 0xff;
        let report = harness_core::exec::verify_replay(&result.trace, &corrupted);
        assert!(!report.matched);
        assert_eq!(report.diverged_at, Some(1));
    }

    #[test]
    fn ephemeral_document_query_via_executor() {
        use crate::orchestration::orchestrate::propose_ephemeral_fallback;
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        let graph = propose_ephemeral_fallback("how many words in the attached document");
        let mut host = MockToolHost::default();
        let mut extra = IndexMap::new();
        extra.insert(
            "document_text".into(),
            Value::Str("one two three four".into()),
        );
        let mut gate = TestGate::default();
        let result = done(
            execute_task_graph(
                &graph,
                "how many words in the attached document",
                &registry,
                &mut gate.context(&mut host),
                &TaskBudget::default_turn(),
                &extra,
            )
            .expect("execute"),
        );
        assert_eq!(result.ephemeral_created, 1);
        assert_eq!(result.ephemeral_destroyed, 1);
    }

    #[test]
    fn client_tool_routed_to_host_not_ephemeral() {
        use crate::orchestration::tool_router::single_tool_graph;

        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry
            .register_tool(read_tool("clients_query", vec![]))
            .unwrap();
        let graph = single_tool_graph(
            "run clients_query",
            registry.tools.get("clients_query").unwrap(),
            ToolRoute::Client,
        );
        let mut host = MockToolHost::default().on("clients_query", |_| {
            Ok(Value::Map(IndexMap::from([(
                "rows".into(),
                Value::List(vec![]),
            )])))
        });
        let mut gate = TestGate::default();
        let result = done(
            execute_task_graph(
                &graph,
                "run clients_query",
                &registry,
                &mut gate.context(&mut host),
                &TaskBudget::default_turn(),
                &IndexMap::new(),
            )
            .expect("execute"),
        );
        assert_eq!(result.ephemeral_created, 0);
        assert_eq!(host.invocation_count_for("clients_query"), Some(1));
    }

    fn effectful_tool(id: &str, params: Vec<ParamSpec>) -> ToolSpec {
        ToolSpec {
            effect: Some(ToolEffect::External),
            effectful: true,
            idempotent: false,
            dry_run_available: false,
            ..read_tool(id, params)
        }
    }

    /// C1: reaching the tenant host from a graph node must pass the policy gate. With no authored
    /// allow, an effectful tool is default-denied and the host is never touched.
    #[test]
    fn effectful_client_node_is_default_denied_without_policy() {
        use crate::orchestration::tool_router::single_tool_graph;

        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry
            .register_tool(effectful_tool("send_otp", vec![]))
            .unwrap();
        let graph = single_tool_graph(
            "send otp",
            registry.tools.get("send_otp").unwrap(),
            ToolRoute::Client,
        );
        let mut host = RecordingHost::default();
        let mut gate = TestGate::default();
        let error = execute_task_graph(
            &graph,
            "send otp",
            &registry,
            &mut gate.context(&mut host),
            &TaskBudget::default_turn(),
            &IndexMap::new(),
        )
        .expect_err("effectful tool must be denied without an explicit allow");
        assert_eq!(error.code, ReasonCode::PolicyDenied);
        assert!(host.calls.is_empty(), "host must never be reached");
    }

    /// H4: a tenant deny on a `std.*` capability is honored, so first-party tools are not a
    /// policy-free side channel.
    #[test]
    fn policy_denial_blocks_a_standard_tool() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        let graph = propose_task_graph(
            "calculate the average of 10 20 30",
            ComplexityClass::Composite,
            &registry,
            &IndexMap::new(),
        )
        .expect("graph");
        let mut host = MockToolHost::default();
        let mut gate = TestGate {
            policies: vec![deny_policy("no-sum", "std.math.sum")],
            ..Default::default()
        };
        let error = execute_task_graph(
            &graph,
            "calculate the average of 10 20 30",
            &registry,
            &mut gate.context(&mut host),
            &TaskBudget::default_turn(),
            &IndexMap::new(),
        )
        .expect_err("denied std tool must not execute");
        assert_eq!(error.code, ReasonCode::PolicyDenied);
    }

    /// C2: the gate derives a per-call idempotency key from user, tool, version, and arguments.
    /// Two calls with different arguments must not collapse onto one key.
    #[test]
    fn distinct_calls_derive_distinct_idempotency_keys() {
        use crate::orchestration::tool_router::single_tool_graph;

        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry
            .register_tool(read_tool("lookup", vec![required_slot_param("q")]))
            .unwrap();
        let graph = single_tool_graph(
            "lookup",
            registry.tools.get("lookup").unwrap(),
            ToolRoute::Client,
        );

        let mut host = RecordingHost::default();
        let mut gate = TestGate::default();
        for term in ["alpha", "beta"] {
            let mut extra = IndexMap::new();
            extra.insert("q".into(), Value::Str(term.into()));
            done(
                execute_task_graph(
                    &graph,
                    "lookup",
                    &registry,
                    &mut gate.context(&mut host),
                    &TaskBudget::default_turn(),
                    &extra,
                )
                .expect("execute"),
            );
        }

        assert_eq!(host.calls.len(), 2);
        assert_ne!(
            host.calls[0].2, host.calls[1].2,
            "distinct arguments must produce distinct idempotency keys"
        );
    }

    /// C3: a declared parameter is bound from the turn slots by the tool's own `ParamSource`,
    /// not handed over as an untyped context blob.
    #[test]
    fn client_tool_binds_declared_param_from_slots() {
        use crate::orchestration::tool_router::single_tool_graph;

        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry
            .register_tool(read_tool("notify", vec![required_slot_param("phone")]))
            .unwrap();
        let graph = single_tool_graph(
            "notify",
            registry.tools.get("notify").unwrap(),
            ToolRoute::Client,
        );
        let mut extra = IndexMap::new();
        extra.insert("phone".into(), Value::Str("+15550100".into()));

        let mut host = RecordingHost::default();
        let mut gate = TestGate::default();
        done(
            execute_task_graph(
                &graph,
                "notify",
                &registry,
                &mut gate.context(&mut host),
                &TaskBudget::default_turn(),
                &extra,
            )
            .expect("execute"),
        );

        let (_, args, _) = &host.calls[0];
        assert_eq!(args.get("phone"), Some(&Value::Str("+15550100".into())));
    }

    /// C3: a required parameter with nothing to bind stops the graph and asks, rather than
    /// invoking the tool with a missing argument.
    #[test]
    fn client_tool_asks_when_required_param_is_unbound() {
        use crate::orchestration::tool_router::single_tool_graph;

        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry
            .register_tool(read_tool("notify", vec![required_slot_param("phone")]))
            .unwrap();
        let graph = single_tool_graph(
            "notify",
            registry.tools.get("notify").unwrap(),
            ToolRoute::Client,
        );

        let mut host = RecordingHost::default();
        let mut gate = TestGate::default();
        let execution = execute_task_graph(
            &graph,
            "notify",
            &registry,
            &mut gate.context(&mut host),
            &TaskBudget::default_turn(),
            &IndexMap::new(),
        )
        .expect("execute");

        match execution {
            GraphExecution::NeedUser { missing, .. } => {
                assert_eq!(missing, vec!["phone".to_string()]);
            }
            GraphExecution::Done(_) => panic!("must not invoke with an unbound required param"),
        }
        assert!(host.calls.is_empty(), "host must never be reached");
    }

    /// H1/H2: the same input executed twice produces a bit-identical ledger. `args_hash` is a
    /// stable content hash and `result` carries no clock- or identity-derived values.
    #[test]
    fn repeat_execution_produces_an_identical_ledger() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        let utterance = "how many words in the attached document";
        let graph = crate::orchestration::orchestrate::propose_ephemeral_fallback(utterance);
        let mut extra = IndexMap::new();
        extra.insert(
            "document_text".into(),
            Value::Str("one two three four".into()),
        );

        let run = |registry: &Registry| {
            let mut host = MockToolHost::default();
            let mut gate = TestGate::default();
            done(
                execute_task_graph(
                    &graph,
                    utterance,
                    registry,
                    &mut gate.context(&mut host),
                    &TaskBudget::default_turn(),
                    &extra,
                )
                .expect("execute"),
            )
        };

        let first = run(&registry);
        let second = run(&registry);
        let fingerprint = |result: &GraphExecutionResult| {
            result
                .ledger
                .iter()
                .map(|e| (e.node_id.clone(), e.args_hash.clone(), e.result.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(fingerprint(&first), fingerprint(&second));
        assert_eq!(first.joined, second.joined);
    }

    /// M1: a goal no std step can serve is refused, so the turn falls through to ProposePath
    /// instead of "answering" the user with their own question.
    #[test]
    fn unserviceable_goal_refuses_instead_of_echoing() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        let utterance = "compare these two options and pick one";
        let graph = crate::orchestration::orchestrate::propose_ephemeral_fallback(utterance);
        let mut host = MockToolHost::default();
        let mut gate = TestGate::default();
        let error = execute_task_graph(
            &graph,
            utterance,
            &registry,
            &mut gate.context(&mut host),
            &TaskBudget::default_turn(),
            &IndexMap::new(),
        )
        .expect_err("ephemeral must refuse rather than echo the utterance");
        assert_eq!(error.code, ReasonCode::Validation);
    }
}
