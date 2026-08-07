//! Turn spine — the hot loop.
//!
//! sense → flow gate → clause split → triage → situation key → tier lookup
//! → [orchestrate] → execute → synthesize → write back
//!
//! **Harness v2 (F-027):** `[orchestrate]` runs on the Tier3 cold path **before** Conductor
//! starters and flat `ProposePath`. It classifies complexity, may emit a verified `TaskGraph`,
//! `TaskGraph`, and lowers each node to Sol or scoped abilities. Warm pin matches and Conductor
//! starters bypass orchestration. See `docs/development/Aelio_harness_v2_multiagent_architecture.md`.

use crate::abilities::express::{self, ExpressVia, Utterance};
use crate::abilities::invoke::CapabilityHost;
use crate::abilities::learn::{
    lookup_tier_with_embedder, observe_and_promote, path_is_effectful, propose_path_greeting,
    propose_path_with_provider, situation_hash, situation_key, typecheck, ObserveOutcome,
    ProposalMap, SituationKey, PROMOTE_MIN_OBSERVATIONS,
};
use crate::abilities::matching::{evaluate_warm_match, MatchingConfig, Outcome};
use crate::abilities::registry::Registry;
use crate::abilities::sense::{sense_budget, sense_env, sense_session, SessionSense};
use crate::abilities::state;
use crate::abilities::understand::{
    classify_depth, classify_depth_with_provider, classify_reply_type, split_clauses,
    split_clauses_with_provider, RepairSignal,
};
use crate::blocks::executor::{
    execute_path, predicate_met, ExecutionFrame, PathExecution, PathExecutionContext,
};
use crate::blocks::flow::{flow_gate, FlowGateResult, FlowInstance};
use crate::contract::AbilityPath;
use crate::ops::effects::EffectEnv;

const PENDING_EFFECT_CONFIRMATION_KEY: &str = "__pending_effect_confirmation_clauses";
use crate::policy::PolicyCtx;
use crate::provider::{LlmProvider, ProviderCall};
use crate::runtime::turn_recall::{RecallBudget, TurnRecall};
use crate::tenant::{
    FlowSpec, ParamConstraint, PersonalitySpec, PolicySpec, StateSpec, TenantDecl,
};
use crate::types::{AelioError, Depth, LookupTier, ReplyType, Value};
use chrono::Utc;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnInput {
    /// Stable durable turn id used to derive child artifact idempotency keys.
    pub turn_id: String,
    pub utterance: String,
    pub user_id: String,
    pub channel: String,
    pub turn_index: u64,
    pub last_seen_secs_ago: Option<u64>,
    pub state_id: String,
    pub slots: IndexMap<String, serde_json::Value>,
    pub active_flow: Option<FlowInstance>,
    pub graph_suspension: Option<crate::orchestration::suspension::GraphSuspension>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnTraceStep {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    pub reply: Utterance,
    pub llm_calls: u32,
    pub tier: Option<LookupTier>,
    pub depth: Depth,
    pub steps: Vec<TurnTraceStep>,
    pub opened_loop: bool,
    pub new_state: Option<String>,
    pub active_flow: Option<FlowInstance>,
    pub situation_hash: Option<String>,
    pub proposal_id: Option<String>,
    pub suspended: bool,
    pub graph_suspension: Option<crate::orchestration::suspension::GraphSuspension>,
}

struct TierPathOutcome {
    reply: Utterance,
    llm_calls: u32,
    opened_loop: bool,
    suspended: bool,
    successful: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ShadowPathOutcome {
    pub evidence_hash: String,
    pub invoked_tools: Vec<String>,
}

pub struct TurnRuntime<'a> {
    pub tenant: &'a TenantDecl,
    pub registry: &'a mut Registry,
    pub policies: &'a [PolicySpec],
    pub states: &'a [StateSpec],
    pub personality: Option<&'a PersonalitySpec>,
    pub effect_env: &'a mut EffectEnv,
    pub proposals: &'a mut ProposalMap,
    pub signatures: &'a mut crate::abilities::sig::SignatureRegistry,
    pub once_seen: &'a mut HashSet<String>,
    pub tool_host: &'a mut dyn CapabilityHost,
    pub adaptive_artifact_host: &'a mut dyn crate::adaptive::AdaptiveArtifactHost,
    pub llm_provider: &'a mut dyn LlmProvider,
    pub turn_recall: &'a mut dyn TurnRecall,
    /// The runtime's configured embedder — bag-of-hash offline, or the gateway-served OpenAI /
    /// Anthropic / Gemini model in production. Every semantic hot-path decision (Tier-1 σ near-match,
    /// intent classification, term bridging, promotion embedding) goes through this one handle.
    pub embedder: &'a dyn crate::embedding::Embedder,
    pub inline_learning_enabled: bool,
    /// Red-rule switch: disables all Tier0/Tier1 dispatches and cold-authors every request.
    pub matching_config: MatchingConfig,
    pub legacy_flow_execution_enabled: bool,
    pub(crate) durable_memory_enabled: bool,
    /// Session harness stack + context pages (Conductor architecture).
    pub harness: &'a mut crate::harness::HarnessSession,
    /// Stored (default) vs Hardcoded benchmark control for starter bodies.
    pub harness_play_mode: crate::harness::HarnessPlayMode,
}

impl<'a> TurnRuntime<'a> {
    pub fn run(&mut self, input: &TurnInput) -> TurnResult {
        let mut steps = Vec::new();
        let mut llm_calls = 0u32;

        // ── Harness stack control (before waiting-child resume) ────────────
        let stack_control = crate::harness::detect_stack_control(&input.utterance);
        match stack_control {
            crate::harness::StackControl::Fresh => {
                self.harness.fresh(true);
                steps.push(TurnTraceStep {
                    name: "Harness.Stack".into(),
                    detail: "fresh: cleared stack and context pages; ceiling=conductor".into(),
                });
            }
            crate::harness::StackControl::ExitUp => {
                let popped = self.harness.exit_up();
                steps.push(TurnTraceStep {
                    name: "Harness.Stack".into(),
                    detail: match popped {
                        Some(frame) => format!(
                            "exit_up: popped {} remaining_depth={}",
                            frame.harness_id,
                            self.harness.stack.len()
                        ),
                        None => "exit_up: stack already empty".into(),
                    },
                });
            }
            crate::harness::StackControl::Stay => {
                if let Some(top) = self.harness.top() {
                    steps.push(TurnTraceStep {
                        name: "Harness.Stack".into(),
                        detail: format!("stay: top={} waiting={}", top.harness_id, top.waiting),
                    });
                }
            }
        }
        let abandon_waiting_child = matches!(
            stack_control,
            crate::harness::StackControl::Fresh | crate::harness::StackControl::ExitUp
        );

        // Waiting child owns the utterance when Stay (no active runtime subject yet).
        if !abandon_waiting_child {
            if let Some(top) = self.harness.waiting_child().cloned() {
                if top.harness_id == crate::harness::WAIT_FOR_USER_ID {
                    return self.resume_wait_for_user(input, steps);
                }
            }
        }

        let active_runtime_subject = match self
            .adaptive_artifact_host
            .has_active_subject(&self.tenant.tenant_id, &input.user_id)
        {
            Ok(active) => active,
            Err(error) => {
                steps.push(TurnTraceStep {
                    name: "FlowGate.Runtime".into(),
                    detail: format!("probe_failed reason={:?}", error.code),
                });
                return rejected_path_turn(
                    steps,
                    "The active runtime flow could not be inspected safely.",
                );
            }
        };
        if active_runtime_subject && abandon_waiting_child {
            if let Some(top) = self.harness.top_mut() {
                top.waiting = false;
            }
            steps.push(TurnTraceStep {
                name: "Harness.Stack".into(),
                detail: "abandoned waiting runtime subject due to exit_up/fresh; not resumed"
                    .into(),
            });
        } else if active_runtime_subject {
            if self.matching_flow(&input.utterance, input).is_some() {
                steps.push(TurnTraceStep {
                    name: "FlowGate.Defer".into(),
                    detail: "second flow intent deferred; active runtime continuation unchanged"
                        .into(),
                });
                return deferred_runtime_flow_turn(steps);
            }
            if let Some((decision, procedure_id)) = self.declared_read_only_detour(input) {
                steps.push(TurnTraceStep {
                    name: "FlowGate.Detour".into(),
                    detail: "declared non-instantiating read-only pathway; continuation unchanged"
                        .into(),
                });
                return self.execute_adaptive_artifact(
                    input,
                    Some(decision),
                    LookupTier::Tier1,
                    Depth::Boundary,
                    None,
                    Some(procedure_id),
                    steps,
                    0,
                );
            } else {
                let projected_wake = serde_json::json!({
                    "turn": {
                        "utterance": input.utterance,
                        "channel": input.channel,
                        "state_id": input.state_id,
                    }
                });
                let resumed = self.adaptive_artifact_host.resume_subject(
                    &self.tenant.tenant_id,
                    &input.user_id,
                    projected_wake,
                );
                if let Some(trace) = self.adaptive_artifact_host.take_decision_trace() {
                    steps.push(TurnTraceStep {
                        name: "Artifact.Ledger".into(),
                        detail: trace,
                    });
                }
                match resumed {
                    Ok(Some(outcome)) => {
                        let suspended = outcome.suspended;
                        let Some(reply) = adaptive_output_utterance(outcome.output) else {
                            return rejected_path_turn(
                                steps,
                                "The active runtime flow returned an invalid response.",
                            );
                        };
                        steps.push(TurnTraceStep {
                            name: "FlowGate.Runtime".into(),
                            detail: format!(
                                "resumed authority=aelio-runtime suspended={suspended}"
                            ),
                        });
                        return TurnResult {
                            reply,
                            llm_calls: 0,
                            tier: None,
                            depth: Depth::Deep,
                            steps,
                            opened_loop: suspended,
                            new_state: None,
                            active_flow: None,
                            situation_hash: None,
                            proposal_id: None,
                            suspended,
                            graph_suspension: None,
                        };
                    }
                    Ok(None) => {
                        return rejected_path_turn(
                            steps,
                            "The active runtime flow disappeared before it could resume.",
                        );
                    }
                    Err(error) => {
                        steps.push(TurnTraceStep {
                            name: "FlowGate.Runtime".into(),
                            detail: format!("failed reason={:?}", error.code),
                        });
                        return rejected_path_turn(
                            steps,
                            "The active runtime flow could not be resumed safely.",
                        );
                    }
                }
            }
        }

        if input
            .active_flow
            .as_ref()
            .is_some_and(|flow| flow.flow_id == "__aelio.multi_clause_confirmation")
        {
            return self.resume_multi_clause_plan(input, &mut steps);
        }
        // Unified mode persists effect confirmation in the harness session (not legacy user_flows).
        // Skip this intercept when already executing a confirmed clause — otherwise
        // `resume_multi_clause_plan` → `run(confirmed utterance)` re-enters here, sees the still-
        // pending plan, and re-asks "confirm?" forever because the utterance is no longer "yes".
        if !self.legacy_flow_execution_enabled
            && input
                .slots
                .get("__aelio_effect_confirmed")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        {
            if let Some(clauses) = self.pending_effect_confirmation_clauses() {
                let mut patched = input.clone();
                patched.active_flow = Some(multi_clause_confirmation_flow(&clauses));
                return self.resume_multi_clause_plan(&patched, &mut steps);
            }
        }
        if input
            .active_flow
            .as_ref()
            .is_some_and(|flow| flow.flow_id == "__aelio.term_confirmation")
        {
            return self.resume_term_confirmation(input, &mut steps);
        }

        // ── 0. SENSE (pure, free, READ only) ─────────────────────────────
        let now = self.effect_env.now().unwrap_or_else(|_| Utc::now());
        let env = sense_env(now, "UTC", "en-US", 0, input.turn_index, &input.channel);
        let session = sense_session(
            vec![],
            input.active_flow.as_ref().map(|f| f.flow_id.clone()),
            input
                .active_flow
                .as_ref()
                .and_then(|f| f.pending_step.clone()),
            input.last_seen_secs_ago.map(|s| format!("{s}s_ago")),
            None,
        );
        let _budget = sense_budget(100_000, 1500, 10, 0.0);
        let state_node = state::read(self.states, &input.state_id).ok();
        steps.push(TurnTraceStep {
            name: "Sense".into(),
            detail: format!(
                "state={} turn={} pending={:?}",
                input.state_id, input.turn_index, session.pending_step
            ),
        });

        // ── 1. FLOW GATE (deterministic; outranks utterance semantics) ───
        match flow_gate(input.active_flow.as_ref()) {
            FlowGateResult::Resume { mut instance } => {
                steps.push(TurnTraceStep {
                    name: "FlowGate".into(),
                    detail: format!("resume pending={:?}", instance.pending_step),
                });
                let continuation = pending_action_continuation(&instance);
                let result = self.resume_flow(input, &mut instance, &mut steps);
                if !result.suspended {
                    if let Some((clauses, next_index)) = continuation {
                        return self.continue_confirmed_plan(input, clauses, next_index, result);
                    }
                }
                return result;
            }
            FlowGateResult::Activate { flow_id } => {
                steps.push(TurnTraceStep {
                    name: "FlowGate".into(),
                    detail: format!("activate {flow_id}"),
                });
            }
            FlowGateResult::ContinueFree => {
                steps.push(TurnTraceStep {
                    name: "FlowGate".into(),
                    detail: "continue free".into(),
                });
            }
        }

        // This is an explicit control request, not an inference problem. Avoid an LLM call and
        // let the durable wrapper policy-check, validate, and commit the fact.
        if crate::memory::explicit_fact(&input.utterance).is_some()
            || crate::memory::explicit_forget_fact(&input.utterance).is_some()
        {
            let available = self.durable_memory_enabled;
            let operation = if crate::memory::explicit_fact(&input.utterance).is_some() {
                "store"
            } else {
                "forget"
            };
            steps.push(TurnTraceStep {
                name: "Memory.Request".into(),
                detail: if available {
                    format!(
                        "explicit durable memory {operation} request; awaiting policy and commit"
                    )
                } else {
                    "rejected: durable memory store unavailable".into()
                },
            });
            return TurnResult {
                reply: if available {
                    express::template(
                        "memory_pending",
                        if operation == "store" {
                            "I'll remember that."
                        } else {
                            "I'll forget that."
                        },
                        &IndexMap::new(),
                    )
                } else {
                    express::apologize(
                        "Durable memory is not available in this runtime",
                        "Use the durable runtime before asking me to remember facts.",
                    )
                },
                llm_calls: 0,
                tier: None,
                depth: Depth::Deep,
                steps,
                opened_loop: false,
                new_state: None,
                active_flow: None,
                situation_hash: None,
                proposal_id: None,
                suspended: false,
                graph_suspension: None,
            };
        }

        // ── 2. CLAUSE SPLIT (pure pre-gate) ───────────────────────────────
        let (_, pure_clauses) = split_clauses(&input.utterance);
        let (clauses, split_detail) =
            if pure_clauses.len() > 1 && self.llm_provider.supports_language_intelligence() {
                let before = self.provider_llm_calls();
                match split_clauses_with_provider(&input.utterance, self.llm_provider) {
                    Ok((clauses, prompt_hash)) => {
                        llm_calls += self.provider_llm_calls().saturating_sub(before) as u32;
                        let count = clauses.len();
                        (
                            clauses,
                            format!("mode=model n={count} prompt_hash={prompt_hash}"),
                        )
                    }
                    Err(error) => {
                        llm_calls += self.provider_llm_calls().saturating_sub(before) as u32;
                        let count = pure_clauses.len();
                        (
                            pure_clauses,
                            format!("mode=pure_fallback n={count} reason={:?}", error.code),
                        )
                    }
                }
            } else {
                let count = pure_clauses.len();
                (pure_clauses, format!("mode=pure n={count}"))
            };
        steps.push(TurnTraceStep {
            name: "SplitClauses".into(),
            detail: split_detail,
        });
        // Never silently discard secondary requests. Conflict and ordering semantics are still
        // tenant decisions, so ask explicitly instead of pretending only clause one existed.
        //
        // But the MVP splitter is a crude marker split (" and " / " but " / …) with no linguistic
        // judgement — `Understand.SplitClauses` is an LLM ability precisely because clause vs. noun
        // conjunction needs it. So gate only when two or more segments each independently read as a
        // request ("cancel my order and send a receipt"), never on a bare noun conjunction ("show my
        // orders and invoices"), which would ask the user to disambiguate a perfectly clear ask.
        let request_clauses: Vec<String> = clauses
            .iter()
            .filter(|clause| {
                self.classify_declared_intent(
                    clause,
                    &state_node
                        .as_ref()
                        .map(|state| state.permission_envelope.clone())
                        .unwrap_or_default(),
                )
                .is_some_and(|intent| intent.confident)
                    || crate::blocks::term_resolve::parse_rank_query(clause).is_some()
                    || clause_looks_actionable(clause)
            })
            .cloned()
            .collect();
        if request_clauses.len() > 1 {
            let mut result = self.run_multi_clause(input, &request_clauses, steps);
            result.llm_calls = result.llm_calls.saturating_add(llm_calls);
            return result;
        }
        // Only one actionable request (or none): treat the whole utterance as a single clause so a
        // noun conjunction is not truncated to its first segment.
        let clause = if clauses.len() > 1 {
            input.utterance.as_str()
        } else {
            clauses
                .first()
                .map(|s| s.as_str())
                .unwrap_or(&input.utterance)
        };

        // Authored *executable* flow triggers outrank semantic depth. Semantic-only catalog flows
        // (no lowering) are install guidance, not OS apps — skip them and continue triage.
        if let Some(flow) = self.matching_flow(clause, input) {
            if let Some(artifact) = self.registry.flow_artifact(&flow.id).cloned() {
                steps.push(TurnTraceStep {
                    name: "FlowMatch".into(),
                    detail: format!("matched installed {} before semantic triage", flow.id),
                });
                let margin = crate::blocks::flow::trigger_surface_match(
                    clause,
                    &flow.activation.trigger_surface,
                );
                let projected_input = serde_json::json!({
                    "turn": {
                        "utterance": input.utterance,
                        "channel": input.channel,
                        "state_id": input.state_id,
                    }
                });
                let decision = crate::adaptive::AdaptiveDecisionEnvelopeV1::seal(
                    vec![crate::adaptive::ArtifactCandidateV1 {
                        artifact: artifact.clone(),
                        score: margin.clamp(-1.0, 1.0),
                        reason: crate::adaptive::CandidateReasonV1::AuthoredFlowActivation,
                    }],
                    crate::adaptive::AdaptiveDecisionV1::Invoke {
                        artifact,
                        projected_input,
                    },
                );
                let mut result = self.execute_bound_flow(input, &flow, decision, &mut steps);
                result.llm_calls = result.llm_calls.saturating_add(llm_calls);
                return result;
            }
            if flow.lowering.is_none() {
                if !self.legacy_flow_execution_enabled {
                    steps.push(TurnTraceStep {
                        name: "FlowMatch".into(),
                        detail: format!(
                            "skipped semantic-only {} (no lowering; not part of install)",
                            flow.id
                        ),
                    });
                } else {
                    // Demo/parity: authored flow steps still run when legacy execution is enabled.
                    steps.push(TurnTraceStep {
                        name: "FlowMatch".into(),
                        detail: format!(
                            "matched {} before semantic triage (legacy; no lowering)",
                            flow.id
                        ),
                    });
                    let mut result = self.activate_flow(input, &flow, &mut steps);
                    result.llm_calls = result.llm_calls.saturating_add(llm_calls);
                    return result;
                }
            } else if !self.legacy_flow_execution_enabled {
                steps.push(TurnTraceStep {
                    name: "FlowMatch".into(),
                    detail: format!("matched {} before semantic triage", flow.id),
                });
                let effectful =
                    flow.steps
                        .iter()
                        .flat_map(|step| &step.admissible)
                        .any(|capability| {
                            self.registry
                                .lookup_tool_by_capability(capability)
                                .iter()
                                .any(|tool| tool.effectful)
                        });
                let demand = crate::adaptive::AdaptiveFlowDemandV1 {
                    flow_id: flow.id.clone(),
                    flow_version: flow.version.clone(),
                    effectful,
                };
                let recorded = demand.validate().and_then(|_| {
                    self.adaptive_artifact_host
                        .record_flow_demand(&self.tenant.tenant_id, &demand)
                });
                steps.push(TurnTraceStep {
                    name: "FlowMaterialization".into(),
                    detail: match recorded {
                        Ok(()) => "missing runtime artifact; demand recorded off-path".into(),
                        Err(error) => format!(
                            "missing runtime artifact; demand unavailable reason={:?}",
                            error.code
                        ),
                    },
                });
                return rejected_path_turn(
                    steps,
                    "This flow has not passed runtime materialization and admission yet.",
                );
            } else {
                steps.push(TurnTraceStep {
                    name: "FlowMatch".into(),
                    detail: format!("matched {} before semantic triage", flow.id),
                });
                let mut result = self.activate_flow(input, &flow, &mut steps);
                result.llm_calls = result.llm_calls.saturating_add(llm_calls);
                return result;
            }
        }

        let caps = state_node
            .as_ref()
            .map(|s| s.permission_envelope.clone())
            .unwrap_or_default();
        let preliminary_intent = self
            .classify_declared_intent(clause, &caps)
            .filter(|intent| intent.confident);
        let declared_query_intent = preliminary_intent.as_ref().is_some_and(|intent| {
            self.tenant.attributes.iter().any(|attribute| {
                attribute.query_capability.as_deref() == Some(intent.label.as_str())
            })
        });
        let mut planned_input = input.clone();
        let rank_capability = match self.prepare_rank_query(
            clause,
            declared_query_intent,
            &mut steps,
            &mut llm_calls,
        ) {
            Ok(Some((plan, capability))) => {
                planned_input.slots.insert(
                    "attribute".into(),
                    serde_json::Value::String(plan.attribute),
                );
                planned_input.slots.insert(
                    "direction".into(),
                    serde_json::Value::String(format!("{:?}", plan.direction).to_lowercase()),
                );
                planned_input
                    .slots
                    .insert("limit".into(), serde_json::Value::from(plan.limit));
                Some(capability)
            }
            Ok(None) => None,
            Err(result) => return *result,
        };
        let input = &planned_input;
        let declared_intent = rank_capability
            .clone()
            .or_else(|| preliminary_intent.map(|intent| intent.label));

        // ── 3. TRIAGE ────────────────────────────────────────────────────
        let mut depth_class = declared_intent.as_ref().map_or_else(
            || classify_depth(clause),
            |_| crate::abilities::understand::DepthClass {
                depth: Depth::Deep,
                margin: 1.0,
                substrate: "declared_intent".into(),
            },
        );
        if declared_intent.is_none()
            && depth_class.margin < 0.6
            && self.llm_provider.supports_language_intelligence()
        {
            let before = self.provider_llm_calls();
            match classify_depth_with_provider(clause, self.llm_provider) {
                Ok((model_class, prompt_hash)) => {
                    llm_calls += self.provider_llm_calls().saturating_sub(before) as u32;
                    depth_class = model_class;
                    steps.push(TurnTraceStep {
                        name: "Understand.Depth".into(),
                        detail: format!("closed extraction prompt_hash={prompt_hash}"),
                    });
                }
                Err(error) => {
                    llm_calls += self.provider_llm_calls().saturating_sub(before) as u32;
                    steps.push(TurnTraceStep {
                        name: "Understand.Depth".into(),
                        detail: format!("pure fallback reason={:?}", error.code),
                    });
                }
            }
        }
        steps.push(TurnTraceStep {
            name: "ClassifyDepth".into(),
            detail: format!("{:?} margin={}", depth_class.depth, depth_class.margin),
        });

        // A parked graph owns the next user answer even when that answer is short. Without this
        // gate a valid phone number or approval reply would be consumed by shallow chat before
        // the continuation reached its pending client-tool node.
        if input.graph_suspension.is_none() {
            match depth_class.depth {
                Depth::Shallow => {
                    let mut result =
                        self.shallow_turn(input, &session, &state_node, &mut steps, &env);
                    result.llm_calls = result.llm_calls.saturating_add(llm_calls);
                    return result;
                }
                Depth::Boundary => {
                    return self.boundary_turn(input, &mut steps, &mut llm_calls);
                }
                Depth::Deep => {}
            }
        }

        // ── 4. SITUATION KEY (bucketed; no personality) ───────────────────
        let intent = declared_intent.unwrap_or_else(|| intent_class(clause));
        let sigma = situation_key(
            &input.state_id,
            &intent,
            input.slots.keys().cloned().collect(),
            caps.clone(),
            None,
            input.turn_index,
            input.last_seen_secs_ago,
        );
        let sh = situation_hash(&sigma);
        steps.push(TurnTraceStep {
            name: "SituationKey".into(),
            detail: sh.clone(),
        });

        if let Some(suspension) = input.graph_suspension.clone() {
            if suspension.is_expired(now.timestamp_millis()) {
                self.effect_env.metric_inc("graph_suspension.expired", 1.0);
                steps.push(TurnTraceStep {
                    name: "Orchestrate.Expired".into(),
                    detail: format!(
                        "suspended_turn={} expired_at_ms={}; treating input as fresh",
                        suspension.turn_id, suspension.expires_at_ms
                    ),
                });
            } else if let Some(result) = crate::blocks::orchestrate::try_resume_orchestrated_graph(
                self,
                input,
                &suspension,
                &sh,
                &sigma,
                &mut steps,
                llm_calls,
            ) {
                return result;
            }
        }

        let recalled = match self.turn_recall.retrieve(
            &input.user_id,
            clause,
            now.timestamp_millis(),
            RecallBudget::default(),
        ) {
            Ok(recalled) => recalled,
            Err(error) => {
                steps.push(TurnTraceStep {
                    name: "Recall.Error".into(),
                    detail: format!("retrieval degraded safely: {:?}", error.code),
                });
                Default::default()
            }
        };
        steps.push(TurnTraceStep {
            name: "Recall.Assemble".into(),
            detail: format!(
                "claims={} truncated={} max_hits={} max_chars={}",
                recalled.claims.len(),
                recalled.truncated,
                RecallBudget::default().max_hits,
                RecallBudget::default().max_chars,
            ),
        });
        // Relevant recalled claims may answer an otherwise unclaimed knowledge request. A
        // confidently declared tool intent always wins, so recall can never swallow a catalogued
        // action. This removes the old English QUESTION/MUTATION keyword gate.
        if !recalled.claims.is_empty()
            && self.registry.lookup_tool_by_capability(&intent).is_empty()
        {
            steps.push(TurnTraceStep {
                name: "Recall.Answer".into(),
                detail: format!("grounded in {} recalled claims", recalled.claims.len()),
            });
            let before = self.provider_llm_calls();
            let reply = self.synthesize_claims_or_fallback(&recalled.claims);
            let calls = self.provider_llm_calls().saturating_sub(before) as u32;
            return TurnResult {
                reply,
                llm_calls: llm_calls.saturating_add(calls),
                tier: None,
                depth: Depth::Deep,
                steps,
                opened_loop: false,
                new_state: None,
                active_flow: None,
                situation_hash: Some(sh),
                proposal_id: None,
                suspended: false,
                graph_suspension: None,
            };
        }

        // ── 5. TIER LOOKUP ───────────────────────────────────────────────
        let mut tier = lookup_tier_with_embedder(self.registry, &sigma, self.embedder);
        // Tier 2 attempt if tier3
        if tier.tier == LookupTier::Tier3 {
            let goal = goal_spec(self.registry, &intent, &caps, input);
            if let Some(path) = crate::abilities::learn::compose(
                self.registry,
                &goal,
                crate::abilities::learn::CompositionBudget::default(),
            ) {
                tier.tier = LookupTier::Tier2;
                tier.path = Some(path);
                steps.push(TurnTraceStep {
                    name: "Tier2Compose".into(),
                    detail: "composed path".into(),
                });
            } else if let Some(capability) = rank_capability.as_deref() {
                // A normalized query-plan fragment plus its tenant-declared capability is a typed
                // one-step composition. It enters the same learning/tier ladder as every other
                // path; rank queries are not a permanent side door.
                tier.tier = LookupTier::Tier2;
                tier.path = Some(AbilityPath::seq([capability]));
                steps.push(TurnTraceStep {
                    name: "Tier2Compose".into(),
                    detail: format!("constructed declared query path {capability}"),
                });
            }
        }
        // Tier lookup only selects a candidate. Gate the candidate before it can dispatch any
        // ability/tool; a rejected candidate becomes an ordinary cold-authoring request.
        if matches!(tier.tier, LookupTier::Tier0 | LookupTier::Tier1) {
            let decision = match tier.path.as_ref() {
                Some(path) => evaluate_warm_match(
                    self.matching_config,
                    tier.tier,
                    self.registry,
                    path,
                    clause,
                    &input.slots,
                ),
                None => crate::abilities::matching::MatchDecision {
                    outcome: Outcome::Rejected,
                    gate: crate::abilities::matching::Gate::FailClosed,
                    reason: "warm candidate has no executable path".into(),
                },
            };
            crate::reuse_metrics::record_match_decision(&decision);
            steps.push(TurnTraceStep {
                name: "WarmMatchGate".into(),
                detail: format!(
                    "{:?}/{:?}: {}",
                    decision.gate, decision.outcome, decision.reason
                ),
            });
            if decision.outcome != Outcome::Accepted {
                tier.tier = LookupTier::Tier3;
                tier.path = None;
                tier.procedure_id = None;
                tier.artifact = None;
            }
        }
        crate::reuse_metrics::record_lookup(tier.tier, &sh);
        steps.push(TurnTraceStep {
            name: "LookupTier".into(),
            detail: format!("{:?}", tier.tier),
        });
        let adaptive_decision = record_adaptive_shadow(&tier, &sh, input, &mut steps);

        match tier.tier {
            LookupTier::Tier0 | LookupTier::Tier1 => {
                let procedure_id = tier.procedure_id.clone();
                if tier.artifact.is_some() {
                    return self.execute_adaptive_artifact(
                        input,
                        adaptive_decision,
                        tier.tier,
                        Depth::Deep,
                        Some(sh),
                        procedure_id,
                        steps,
                        llm_calls,
                    );
                }
                let Some(path) = tier.path else {
                    return rejected_path_turn(
                        steps,
                        "The promoted procedure had no executable path.",
                    );
                };
                if let Some(confirmation) =
                    self.confirm_effectful_path(input, &path, &mut steps, llm_calls, tier.tier)
                {
                    return confirmation;
                }
                let executed = self.execute_tier_path(
                    input,
                    &path,
                    &state_node,
                    &caps,
                    &recalled.claims,
                    &mut steps,
                );
                // Shadow exploration is an operator-budgeted durable-runtime concern. The hot
                // path never double-runs a promoted procedure by itself.
                TurnResult {
                    reply: executed.reply,
                    llm_calls: llm_calls.saturating_add(executed.llm_calls),
                    tier: Some(tier.tier),
                    depth: Depth::Deep,
                    steps,
                    opened_loop: executed.opened_loop,
                    new_state: None,
                    active_flow: None,
                    situation_hash: Some(sh),
                    proposal_id: procedure_id,
                    suspended: executed.suspended,
                    graph_suspension: None,
                }
            }
            LookupTier::Tier2 => {
                let Some(path) = tier.path else {
                    return rejected_path_turn(
                        steps,
                        "The composition planner returned no executable path.",
                    );
                };
                if let Err(error) = typecheck(self.registry, &path) {
                    steps.push(TurnTraceStep {
                        name: "TypeCheck".into(),
                        detail: format!("rejected: {error}"),
                    });
                    return rejected_path_turn(steps, "The composed procedure was not type-safe.");
                }
                if let Some(confirmation) =
                    self.confirm_effectful_path(input, &path, &mut steps, llm_calls, tier.tier)
                {
                    return confirmation;
                }
                let executed = self.execute_tier_path(
                    input,
                    &path,
                    &state_node,
                    &caps,
                    &recalled.claims,
                    &mut steps,
                );
                let prop_id = self.learn_from_turn(&sigma, &path, executed.successful, &mut steps);
                TurnResult {
                    reply: executed.reply,
                    llm_calls: llm_calls.saturating_add(executed.llm_calls),
                    tier: Some(LookupTier::Tier2),
                    depth: Depth::Deep,
                    steps,
                    opened_loop: executed.opened_loop,
                    new_state: None,
                    active_flow: None,
                    situation_hash: Some(sh),
                    proposal_id: Some(prop_id),
                    suspended: executed.suspended,
                    graph_suspension: None,
                }
            }
            LookupTier::Tier3 => {
                // Harness v2: composite/open queries decompose before Conductor starters.
                if let Some(result) = crate::blocks::orchestrate::try_orchestrated_cold_path(
                    self, input, clause, &sigma, &sh, &mut steps, llm_calls,
                ) {
                    return result;
                }
                // Conductor chooses a starter harness; escalate alone falls through to ProposePath.
                // A starter can also decline its own answer (e.g. ReplyNumericGuard) and fall
                // through here — count whatever it spent either way.
                let before_conductor_calls = self.provider_llm_calls();
                if let Some(result) =
                    self.run_conductor_starters(input, clause, &sh, &mut steps, llm_calls)
                {
                    return result;
                }
                llm_calls += self
                    .provider_llm_calls()
                    .saturating_sub(before_conductor_calls) as u32;
                steps.push(TurnTraceStep {
                    name: "Conductor".into(),
                    detail: "escalate: cold ProposePath over declared abilities".into(),
                });
                // LLM proposes path over declared abilities. Large catalogs are shortlisted by
                // relevance first — the id list is also ProposePath's allow-list, so narrowing it
                // can only shrink what the model may choose, never widen it.
                let before = self.provider_llm_calls();
                let declared = self.registry.abilities.keys().cloned().collect::<Vec<_>>();
                // Score against the whole utterance, which is what ProposePath is asked about. A
                // single clause is a narrower signal and could drop an ability the rest needs.
                let retrieved = crate::abilities::retrieval::shortlist_abilities(
                    &input.utterance,
                    &declared,
                    self.registry,
                    self.embedder,
                    &crate::abilities::retrieval::RetrievalConfig::default(),
                );
                if retrieved.applied {
                    steps.push(TurnTraceStep {
                        name: "ProposePath.Retrieve".into(),
                        detail: format!(
                            "shortlisted {} of {} declared abilities",
                            retrieved.ability_ids.len(),
                            retrieved.considered
                        ),
                    });
                }
                let ability_ids = retrieved.ability_ids;
                let ability_declarations = ability_ids
                    .iter()
                    .map(|ability_id| {
                        let tools = self
                            .registry
                            .abilities
                            .get(ability_id)
                            .map(|contract| {
                                contract
                                    .tool_deps
                                    .iter()
                                    .filter_map(|tool_id| self.registry.tools.get(tool_id))
                                    .map(|tool| {
                                        serde_json::json!({
                                            "tool_id": tool.id,
                                            "params": tool.params.iter().map(|param| serde_json::json!({
                                                "name": param.name,
                                                "type": param.type_name,
                                                "required": param.required,
                                            })).collect::<Vec<_>>()
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        serde_json::json!({
                            "ability_id": ability_id,
                            "tools": tools,
                        })
                        .to_string()
                    })
                    .collect::<Vec<_>>();
                let proposed = propose_path_with_provider(
                    &sigma,
                    &input.utterance,
                    &ability_ids,
                    &ability_declarations,
                    self.llm_provider,
                );
                llm_calls += self.provider_llm_calls().saturating_sub(before) as u32;
                let (path, proposal_prompt_hash) = match proposed {
                    Ok(proposed) => proposed,
                    Err(error) => {
                        steps.push(TurnTraceStep {
                            name: "ProposePath".into(),
                            detail: format!("failed closed: {:?}", error.code),
                        });
                        let mut rejected = rejected_path_turn(
                            steps,
                            "I couldn't construct a safe declared procedure for that request.",
                        );
                        rejected.llm_calls = llm_calls;
                        return rejected;
                    }
                };
                let proposed_abilities: Vec<String> = path
                    .steps
                    .iter()
                    .map(|step| step.ability_id.clone())
                    .collect();
                steps.push(TurnTraceStep {
                    name: "ProposePath".into(),
                    detail: format!(
                        "{} steps [{}] prompt_hash={proposal_prompt_hash}",
                        path.steps.len(),
                        proposed_abilities.join(", ")
                    ),
                });
                if let Err(error) = typecheck(self.registry, &path) {
                    steps.push(TurnTraceStep {
                        name: "TypeCheck".into(),
                        detail: format!("rejected: {error}"),
                    });
                    return rejected_path_turn(steps, "The proposed procedure was not type-safe.");
                }
                steps.push(TurnTraceStep {
                    name: "TypeCheck".into(),
                    detail: "ok".into(),
                });
                // A path can typecheck cleanly while silently dropping a constraint the
                // utterance stated ("... in Bangalore" binding to a shape that never mentions
                // the city). Heuristic, high-recall by design — see abilities::residue.
                let unconsumed = crate::abilities::residue::residue_check(clause, &path);
                if !unconsumed.is_empty() {
                    let fragments = unconsumed
                        .iter()
                        .map(|f| f.fragment.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    steps.push(TurnTraceStep {
                        name: "ResidueCheck".into(),
                        detail: format!("rejected: unconsumed [{fragments}]"),
                    });
                    return rejected_path_turn(
                        steps,
                        "The proposed procedure didn't account for everything in the request.",
                    );
                }
                steps.push(TurnTraceStep {
                    name: "ResidueCheck".into(),
                    detail: "ok".into(),
                });
                if let Some(confirmation) =
                    self.confirm_effectful_path(input, &path, &mut steps, llm_calls, tier.tier)
                {
                    return confirmation;
                }
                let executed = self.execute_tier_path(
                    input,
                    &path,
                    &state_node,
                    &caps,
                    &recalled.claims,
                    &mut steps,
                );
                llm_calls = llm_calls.saturating_add(executed.llm_calls);
                let prop_id = self.learn_from_turn(&sigma, &path, executed.successful, &mut steps);
                let _ = self.effect_env.ledger_append(
                    "turn",
                    crate::types::Value::Map(indexmap::indexmap! {
                        "sigma".into() => crate::types::Value::str(&sh),
                        "tier".into() => crate::types::Value::str("tier3"),
                    }),
                );
                TurnResult {
                    reply: executed.reply,
                    llm_calls,
                    tier: Some(LookupTier::Tier3),
                    depth: Depth::Deep,
                    steps,
                    opened_loop: executed.opened_loop,
                    new_state: None,
                    active_flow: None,
                    situation_hash: Some(sh),
                    proposal_id: Some(prop_id),
                    suspended: executed.suspended,
                    graph_suspension: None,
                }
            }
        }
    }

    /// Execute a runner-up without producing user-visible speech or changing conversational state.
    /// Only read-only, model-free paths are admissible; suspension is a failed shadow trial.
    pub(crate) fn run_shadow_path(
        &mut self,
        input: &TurnInput,
        path: &AbilityPath,
    ) -> crate::types::AelioResult<ShadowPathOutcome> {
        if path_is_effectful(self.registry, path) {
            return Err(AelioError::new(
                crate::types::ReasonCode::PolicyDenied,
                "shadow exploration cannot execute an effectful path",
            ));
        }
        if path.steps.iter().any(|step| {
            self.registry
                .abilities
                .get(&step.ability_id)
                .is_some_and(|ability| ability.prompt_hash.is_some())
        }) {
            return Err(AelioError::new(
                crate::types::ReasonCode::PolicyDenied,
                "shadow exploration cannot spend an LLM call",
            ));
        }
        let state_node = state::read(self.states, &input.state_id).ok();
        let capabilities = state_node
            .as_ref()
            .map(|state| state.permission_envelope.clone())
            .unwrap_or_default();
        let initial_state = input.state_id.clone();
        let frame = ExecutionFrame {
            slots: input
                .slots
                .iter()
                .map(|(name, value)| (name.clone(), crate::ops::pure::json_to_value(value)))
                .collect(),
            state: Some(initial_state.clone()),
            capabilities,
            ..Default::default()
        };
        match self.execute(path, frame, input, None)? {
            PathExecution::NeedUser { .. } => Err(AelioError::new(
                crate::types::ReasonCode::GateNotMet,
                "shadow candidate required user input",
            )),
            PathExecution::Done {
                frame,
                invoked_tools,
            } => {
                if frame.state.as_deref() != Some(initial_state.as_str()) {
                    return Err(AelioError::new(
                        crate::types::ReasonCode::PolicyDenied,
                        "shadow candidate attempted a state transition",
                    ));
                }
                use sha2::{Digest, Sha256};
                let canonical =
                    serde_json::to_vec(&(frame.evidence, frame.claims)).map_err(|error| {
                        AelioError::new(crate::types::ReasonCode::Internal, error.to_string())
                    })?;
                Ok(ShadowPathOutcome {
                    evidence_hash: hex::encode(&Sha256::digest(canonical)[..16]),
                    invoked_tools,
                })
            }
        }
    }

    fn shallow_turn(
        &mut self,
        input: &TurnInput,
        session: &SessionSense,
        state_node: &Option<state::StateNode>,
        steps: &mut Vec<TurnTraceStep>,
        _env: &crate::abilities::sense::EnvSense,
    ) -> TurnResult {
        let reply_class = classify_reply_type(&input.utterance);
        steps.push(TurnTraceStep {
            name: "ClassifyReplyType".into(),
            detail: format!("{:?}", reply_class.reply_type),
        });

        // Still compute σ + tier for warm greeting path
        let caps = state_node
            .as_ref()
            .map(|s| s.permission_envelope.clone())
            .unwrap_or_default();
        let sigma = situation_key(
            &input.state_id,
            "greeting",
            vec![],
            caps.clone(),
            None,
            input.turn_index,
            input.last_seen_secs_ago,
        );
        let sh = situation_hash(&sigma);
        let tier = lookup_tier_with_embedder(self.registry, &sigma, self.embedder);
        steps.push(TurnTraceStep {
            name: "LookupTier".into(),
            detail: format!("{:?}", tier.tier),
        });
        let returning = matches!(
            session.last_seen.as_deref(),
            Some(s) if !s.is_empty()
        );
        let dormant = matches!(
            crate::abilities::learn::bucket_last_seen(input.last_seen_secs_ago),
            crate::abilities::learn::SeenBucket::Dormant
        );

        if matches!(tier.tier, LookupTier::Tier0 | LookupTier::Tier1) {
            let adaptive_decision = record_adaptive_shadow(&tier, &sh, input, steps);
            let procedure_id = tier.procedure_id.clone();
            if tier.artifact.is_some() {
                return self.execute_adaptive_artifact(
                    input,
                    adaptive_decision,
                    tier.tier,
                    Depth::Shallow,
                    Some(sh),
                    procedure_id,
                    steps.clone(),
                    0,
                );
            }
            let Some(path) = tier.path else {
                return rejected_path_turn(
                    steps.clone(),
                    "The promoted greeting procedure had no executable path.",
                );
            };
            let reply =
                self.execute_greeting_path(input, &path, state_node, &caps, returning, dormant);
            steps.push(TurnTraceStep {
                name: "ExecutePromoted".into(),
                detail: "0 LLM".into(),
            });
            return TurnResult {
                reply,
                llm_calls: 0,
                tier: Some(tier.tier),
                depth: Depth::Shallow,
                steps: steps.clone(),
                opened_loop: false,
                new_state: None,
                active_flow: None,
                situation_hash: Some(sh),
                proposal_id: procedure_id,
                suspended: false,
                graph_suspension: None,
            };
        }

        // Greeting structure is authored and deterministic; only output-oriented banter needs
        // synthesis intelligence. Prefer Conductor starter harnesses over rediscovering a path.
        let before_conductor_calls = self.provider_llm_calls();
        if let Some(mut result) =
            self.run_conductor_starters(input, &input.utterance, &sh, steps, 0)
        {
            result.depth = Depth::Shallow;
            return result;
        }
        let path = propose_path_greeting();
        let mut llm_calls = self
            .provider_llm_calls()
            .saturating_sub(before_conductor_calls) as u32;
        steps.push(TurnTraceStep {
            name: "Tier2Compose".into(),
            detail: "authored generic greeting path".into(),
        });
        if let Err(error) = typecheck(self.registry, &path) {
            steps.push(TurnTraceStep {
                name: "TypeCheck".into(),
                detail: format!("rejected: {error}"),
            });
            return rejected_path_turn(
                steps.clone(),
                "The proposed greeting path was not type-safe.",
            );
        }
        steps.push(TurnTraceStep {
            name: "TypeCheck".into(),
            detail: "ok".into(),
        });
        record_adaptive_shadow(
            &crate::abilities::learn::TierLookup {
                tier: LookupTier::Tier2,
                procedure_id: None,
                path: Some(path.clone()),
                margin: 0.0,
                artifact: None,
            },
            &sh,
            input,
            steps,
        );
        let reply = if reply_class.reply_type == ReplyType::Generic {
            // After proposal, can still template
            express::greeting_template(
                self.personality,
                state_node
                    .as_ref()
                    .and_then(|s| s.direction_target.as_deref()),
                &caps_labels(&caps),
                returning,
                dormant,
            )
        } else {
            let before = self.provider_llm_calls();
            let utterance = self.synthesize_or_fallback("Hello!");
            llm_calls += self.provider_llm_calls().saturating_sub(before) as u32;
            utterance
        };
        let prop_id = self.learn_from_turn(&sigma, &path, true, steps);
        TurnResult {
            reply,
            llm_calls,
            tier: Some(LookupTier::Tier2),
            depth: Depth::Shallow,
            steps: steps.clone(),
            opened_loop: false,
            new_state: None,
            active_flow: None,
            situation_hash: Some(sh),
            proposal_id: Some(prop_id),
            suspended: false,
            graph_suspension: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_adaptive_artifact(
        &mut self,
        input: &TurnInput,
        decision: Option<crate::adaptive::AdaptiveDecisionEnvelopeV1>,
        tier: LookupTier,
        depth: Depth,
        situation_hash: Option<String>,
        procedure_id: Option<String>,
        mut steps: Vec<TurnTraceStep>,
        llm_calls: u32,
    ) -> TurnResult {
        let Some(decision) = decision else {
            return rejected_path_turn(steps, "The adaptive decision was invalid.");
        };
        let invoked =
            self.adaptive_artifact_host
                .invoke(&self.tenant.tenant_id, &input.turn_id, &decision);
        if let Some(trace) = self.adaptive_artifact_host.take_decision_trace() {
            steps.push(TurnTraceStep {
                name: "Artifact.Ledger".into(),
                detail: trace,
            });
        }
        match invoked {
            Ok(outcome) => {
                let suspended = outcome.suspended;
                let Some(reply) = adaptive_output_utterance(outcome.output) else {
                    steps.push(TurnTraceStep {
                        name: "AdaptiveInvoke".into(),
                        detail: "rejected invalid closed output".into(),
                    });
                    return rejected_path_turn(
                        steps,
                        "The admitted procedure returned an invalid response.",
                    );
                };
                steps.push(TurnTraceStep {
                    name: "AdaptiveInvoke".into(),
                    detail: format!(
                        "completed decision_hash={} authority=aelio-runtime",
                        decision.decision_hash
                    ),
                });
                TurnResult {
                    reply,
                    llm_calls,
                    tier: Some(tier),
                    depth,
                    steps,
                    opened_loop: suspended,
                    new_state: None,
                    active_flow: None,
                    situation_hash,
                    proposal_id: procedure_id,
                    suspended,
                    graph_suspension: None,
                }
            }
            Err(error) => {
                steps.push(TurnTraceStep {
                    name: "AdaptiveInvoke".into(),
                    detail: format!("failed reason={:?}", error.code),
                });
                rejected_path_turn(
                    steps,
                    "The admitted procedure could not be executed safely.",
                )
            }
        }
    }

    fn boundary_turn(
        &mut self,
        input: &TurnInput,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: &mut u32,
    ) -> TurnResult {
        // Conductor owns ambiguous boundary turns when a starter harness fits.
        let before_conductor_calls = self.provider_llm_calls();
        if let Some(mut result) =
            self.run_conductor_starters(input, &input.utterance, "boundary", steps, *llm_calls)
        {
            result.depth = Depth::Boundary;
            return result;
        }
        *llm_calls += self
            .provider_llm_calls()
            .saturating_sub(before_conductor_calls) as u32;
        let recalled = self
            .turn_recall
            .retrieve(
                &input.user_id,
                &input.utterance,
                Utc::now().timestamp_millis(),
                RecallBudget::default(),
            )
            .unwrap_or_default();
        let mut claims = recalled.claims;
        claims.push(crate::abilities::judge::EvidenceClaim {
            id: "runtime.state".into(),
            value: Value::str(&input.state_id),
            provenance: "current_session_state".into(),
        });
        let capabilities = state::read(self.states, &input.state_id)
            .map(|node| node.permission_envelope)
            .unwrap_or_default();
        claims.push(crate::abilities::judge::EvidenceClaim {
            id: "runtime.capabilities".into(),
            value: Value::List(capabilities.iter().map(Value::str).collect()),
            provenance: "active_tenant_catalog".into(),
        });
        steps.push(TurnTraceStep {
            name: "Recall.Assemble".into(),
            detail: format!(
                "boundary claims={} truncated={}",
                claims.len(),
                recalled.truncated
            ),
        });
        steps.push(TurnTraceStep {
            name: "BoundaryAnswer".into(),
            detail: format!(
                "single synthesis prompt_hash={}",
                express::synthesize_prompt_spec()
                    .canonical_hash()
                    .unwrap_or_else(|_| "invalid-prompt-spec".into())
            ),
        });
        let before = self.provider_llm_calls();
        let reply = self.synthesize_claims_or_fallback(&claims);
        *llm_calls += self.provider_llm_calls().saturating_sub(before) as u32;
        TurnResult {
            reply,
            llm_calls: *llm_calls,
            tier: None,
            depth: Depth::Boundary,
            steps: steps.clone(),
            opened_loop: false,
            new_state: None,
            active_flow: None,
            situation_hash: None,
            proposal_id: None,
            suspended: false,
            graph_suspension: None,
        }
    }

    fn execute_greeting_path(
        &mut self,
        input: &TurnInput,
        path: &AbilityPath,
        state_node: &Option<state::StateNode>,
        caps: &[String],
        returning: bool,
        dormant: bool,
    ) -> Utterance {
        let frame = ExecutionFrame {
            state: Some(input.state_id.clone()),
            direction: state_node
                .as_ref()
                .and_then(|node| node.direction_target.clone()),
            capabilities: caps.to_vec(),
            returning,
            dormant,
            ..Default::default()
        };
        match self.execute(path, frame, input, None) {
            Ok(PathExecution::Done { frame, .. }) => frame.reply.unwrap_or_else(|| {
                express::greeting_template(
                    self.personality,
                    state_node
                        .as_ref()
                        .and_then(|node| node.direction_target.as_deref()),
                    &caps_labels(caps),
                    returning,
                    dormant,
                )
            }),
            _ => express::greeting_template(
                self.personality,
                state_node
                    .as_ref()
                    .and_then(|node| node.direction_target.as_deref()),
                &caps_labels(caps),
                returning,
                dormant,
            ),
        }
    }

    /// Execute a retrieved/composed/proposed path through the same generic typed executor.
    /// Terminal speech is derived from the resulting safe evidence; it is never replaced by a
    /// greeting merely because the path came from a particular lookup tier.
    fn execute_tier_path(
        &mut self,
        input: &TurnInput,
        path: &AbilityPath,
        state_node: &Option<state::StateNode>,
        caps: &[String],
        recalled_claims: &[crate::abilities::judge::EvidenceClaim],
        steps: &mut Vec<TurnTraceStep>,
    ) -> TierPathOutcome {
        let frame = ExecutionFrame {
            slots: input
                .slots
                .iter()
                .map(|(name, value)| (name.clone(), crate::ops::pure::json_to_value(value)))
                .collect(),
            state: Some(input.state_id.clone()),
            direction: state_node
                .as_ref()
                .and_then(|node| node.direction_target.clone()),
            capabilities: caps.to_vec(),
            ..Default::default()
        };
        match self.execute(path, frame, input, None) {
            Ok(PathExecution::Done {
                frame,
                invoked_tools,
            }) => {
                for tool in invoked_tools {
                    steps.push(TurnTraceStep {
                        name: "Invoke.Call".into(),
                        detail: format!("{tool} success"),
                    });
                }
                for trace in &frame.artifact_traces {
                    steps.push(TurnTraceStep {
                        name: "Artifact.Ledger".into(),
                        detail: trace.to_string(),
                    });
                }
                if let Some(reply) = frame.reply {
                    return TierPathOutcome {
                        reply,
                        llm_calls: 0,
                        opened_loop: false,
                        suspended: false,
                        successful: true,
                    };
                }
                let mut claims = recalled_claims.to_vec();
                claims.extend(frame.claims.clone());
                for (name, value) in &frame.evidence {
                    let id = format!("execution.{name}");
                    if !claims.iter().any(|claim| claim.id == id)
                        && value.as_str() != Some("[REDACTED]")
                    {
                        claims.push(crate::abilities::judge::EvidenceClaim {
                            id,
                            value: value.clone(),
                            provenance: "verified_execution_frame".into(),
                        });
                    }
                }
                if claims.is_empty() {
                    claims.push(crate::abilities::judge::EvidenceClaim {
                        id: "turn.request".into(),
                        value: Value::str(&input.utterance),
                        provenance: "user_utterance".into(),
                    });
                }
                let before = self.provider_llm_calls();
                let reply = self.synthesize_claims_or_fallback(&claims);
                let llm_calls = self.provider_llm_calls().saturating_sub(before) as u32;
                TierPathOutcome {
                    reply,
                    llm_calls,
                    opened_loop: false,
                    suspended: false,
                    successful: true,
                }
            }
            Ok(PathExecution::NeedUser { question, .. }) => TierPathOutcome {
                reply: Utterance {
                    text: question,
                    via: ExpressVia::Ask,
                    template_id: None,
                    claim_refs: vec![],
                    frame: None,
                },
                llm_calls: 0,
                opened_loop: true,
                suspended: true,
                successful: false,
            },
            Err(error) => {
                if let Some(trace) = &error.decision_trace {
                    steps.push(TurnTraceStep {
                        name: "Artifact.Ledger".into(),
                        detail: trace.to_string(),
                    });
                }
                steps.push(TurnTraceStep {
                    name: "Execute".into(),
                    detail: trace_safe_error_detail(&error),
                });
                TierPathOutcome {
                    reply: express::apologize(
                        "I couldn't safely complete that operation.",
                        "Please try again or contact support.",
                    ),
                    llm_calls: 0,
                    opened_loop: false,
                    suspended: false,
                    successful: false,
                }
            }
        }
    }

    fn execute(
        &mut self,
        path: &AbilityPath,
        frame: ExecutionFrame,
        input: &TurnInput,
        flow_id: Option<&str>,
    ) -> Result<PathExecution, AelioError> {
        let policy = PolicyCtx {
            state: Some(input.state_id.clone()),
            tenant: Some(self.tenant.tenant_id.clone()),
            flow_id: flow_id.map(str::to_owned),
            slots: frame.slots.clone(),
            ..Default::default()
        };
        let mut effect_seq = 0;
        execute_path(
            path,
            frame,
            &mut PathExecutionContext {
                registry: self.registry,
                policies: self.policies,
                policy,
                personality: self.personality,
                host: self.tool_host,
                signatures: self.signatures,
                once_seen: self.once_seen,
                effects: self.effect_env,
                user_id: &input.user_id,
                channel: &input.channel,
                turn_key: &input.turn_id,
                effect_seq: &mut effect_seq,
            },
        )
    }

    fn matching_flow(&self, utterance: &str, input: &TurnInput) -> Option<FlowSpec> {
        self.registry
            .flows
            .values()
            .filter_map(|flow| {
                let margin = crate::blocks::flow::trigger_surface_match(
                    utterance,
                    &flow.activation.trigger_surface,
                );
                let ctx = PolicyCtx {
                    state: Some(input.state_id.clone()),
                    tenant: Some(self.tenant.tenant_id.clone()),
                    flow_id: Some(flow.id.clone()),
                    ..Default::default()
                };
                crate::blocks::flow::activate_flow(flow, self.policies, &ctx, margin)
                    .ok()
                    .map(|_| (margin, flow.clone()))
            })
            .max_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, flow)| flow)
    }

    fn declared_read_only_detour(
        &self,
        input: &TurnInput,
    ) -> Option<(crate::adaptive::AdaptiveDecisionEnvelopeV1, String)> {
        let permission_envelope = self
            .states
            .iter()
            .find(|state| state.id == input.state_id)
            .map(|state| state.permission_envelope.clone())
            .unwrap_or_default();
        let reachable = self.registry.reachable_capabilities(&permission_envelope);
        let intent_label = self
            .classify_declared_intent(&input.utterance, &reachable)
            .map(|intent| intent.label)
            .or_else(|| {
                crate::abilities::understand::is_exact_greeting(&input.utterance)
                    .then(|| "greeting".into())
            })
            .or_else(|| {
                (classify_depth(&input.utterance).depth == Depth::Boundary)
                    .then(|| "boundary".into())
            });
        let intent_label = intent_label?;
        let eligible = self.registry.procedures.values().filter(|procedure| {
            procedure.status == crate::abilities::registry::ProcedureStatus::Promoted
                && procedure
                    .situation_filter
                    .state
                    .as_deref()
                    .is_none_or(|state| state == input.state_id)
                && procedure.situation_filter.intent_class.as_deref() == Some(&intent_label)
                && self.registry.procedure_artifact(&procedure.id).is_some()
        });
        let mut selected = None;
        for procedure in eligible {
            let instantiates_flow = procedure
                .path
                .steps
                .iter()
                .any(|step| step.ability_id.starts_with("Flow."));
            let effectful_dependency = procedure.tool_deps.iter().any(|tool_id| {
                self.registry
                    .tools
                    .get(tool_id)
                    .is_none_or(|tool| tool.effectful)
            });
            if instantiates_flow
                || procedure.contract.effectful
                || effectful_dependency
                || path_is_effectful(self.registry, &procedure.path)
            {
                return None;
            }
            if selected.is_none() {
                let artifact = self.registry.procedure_artifact(&procedure.id)?.clone();
                selected = Some((procedure.id.clone(), artifact));
            }
        }
        let (procedure_id, artifact) = selected?;
        let projected_input = serde_json::json!({
            "turn": {
                "utterance": input.utterance,
                "channel": input.channel,
                "state_id": input.state_id,
            }
        });
        crate::adaptive::AdaptiveDecisionEnvelopeV1::seal(
            vec![crate::adaptive::ArtifactCandidateV1 {
                artifact: artifact.clone(),
                score: 1.0,
                reason: crate::adaptive::CandidateReasonV1::NearSituation,
            }],
            crate::adaptive::AdaptiveDecisionV1::Invoke {
                artifact,
                projected_input,
            },
        )
        .ok()
        .map(|decision| (decision, procedure_id))
    }

    fn activate_flow(
        &mut self,
        input: &TurnInput,
        flow: &FlowSpec,
        steps: &mut Vec<TurnTraceStep>,
    ) -> TurnResult {
        steps.push(TurnTraceStep {
            name: "ActivateFlow".into(),
            detail: flow.id.clone(),
        });
        let mut instance = crate::blocks::flow::start_instance(flow);
        self.run_flow_step(input, flow, &mut instance, steps)
    }

    fn execute_bound_flow(
        &mut self,
        input: &TurnInput,
        flow: &FlowSpec,
        decision: crate::types::AelioResult<crate::adaptive::AdaptiveDecisionEnvelopeV1>,
        steps: &mut Vec<TurnTraceStep>,
    ) -> TurnResult {
        let Ok(decision) = decision else {
            return rejected_path_turn(
                steps.clone(),
                "The authored flow activation decision was invalid.",
            );
        };
        let started = self.adaptive_artifact_host.start_subject(
            &self.tenant.tenant_id,
            &input.user_id,
            &input.turn_id,
            &decision,
        );
        if let Some(trace) = self.adaptive_artifact_host.take_decision_trace() {
            steps.push(TurnTraceStep {
                name: "Artifact.Ledger".into(),
                detail: trace,
            });
        }
        match started {
            Ok(outcome) => {
                let suspended = outcome.suspended;
                let Some(reply) = adaptive_output_utterance(outcome.output) else {
                    return rejected_path_turn(
                        steps.clone(),
                        "The runtime-owned flow returned an invalid response.",
                    );
                };
                steps.push(TurnTraceStep {
                    name: "ActivateFlow.Runtime".into(),
                    detail: format!(
                        "flow={} authority=aelio-runtime suspended={} decision_hash={}",
                        flow.id, suspended, decision.decision_hash
                    ),
                });
                TurnResult {
                    reply,
                    llm_calls: 0,
                    tier: None,
                    depth: Depth::Deep,
                    steps: steps.clone(),
                    opened_loop: suspended,
                    new_state: None,
                    active_flow: None,
                    situation_hash: None,
                    proposal_id: None,
                    suspended,
                    graph_suspension: None,
                }
            }
            Err(error) => {
                steps.push(TurnTraceStep {
                    name: "ActivateFlow.Runtime".into(),
                    detail: format!("failed reason={:?}", error.code),
                });
                rejected_path_turn(
                    steps.clone(),
                    "The admitted authored flow could not be started safely.",
                )
            }
        }
    }

    fn resume_flow(
        &mut self,
        input: &TurnInput,
        instance: &mut FlowInstance,
        steps: &mut Vec<TurnTraceStep>,
    ) -> TurnResult {
        steps.push(TurnTraceStep {
            name: "ResumeFlow".into(),
            detail: "re-check policy+state+versions".into(),
        });
        let Ok(flow) = self.registry.lookup_flow(&instance.flow_id).cloned() else {
            return failed_turn("The active flow is no longer registered.", instance, steps);
        };
        let versions = self
            .registry
            .tools
            .iter()
            .map(|(id, tool)| (id.clone(), tool.version.clone()))
            .collect();
        let prompt_hashes: IndexMap<String, String> = self
            .registry
            .abilities
            .iter()
            .filter_map(|(id, ability)| ability.prompt_hash.clone().map(|hash| (id.clone(), hash)))
            .collect();
        let ctx = PolicyCtx {
            state: Some(input.state_id.clone()),
            tenant: Some(self.tenant.tenant_id.clone()),
            flow_id: Some(flow.id.clone()),
            ..Default::default()
        };
        match crate::blocks::flow::resume_flow(
            instance,
            &flow,
            self.policies,
            &ctx,
            &versions,
            &prompt_hashes,
        ) {
            Ok(crate::blocks::flow::ResumeVerdict::Continue) => {
                self.run_flow_step(input, &flow, instance, steps)
            }
            Ok(crate::blocks::flow::ResumeVerdict::Escape(_)) | Err(_) => {
                failed_turn("The flow can no longer be resumed safely.", instance, steps)
            }
        }
    }

    fn run_flow_step(
        &mut self,
        input: &TurnInput,
        flow: &FlowSpec,
        instance: &mut FlowInstance,
        steps: &mut Vec<TurnTraceStep>,
    ) -> TurnResult {
        let Some(step) = flow.steps.get(instance.current_step_idx) else {
            return failed_turn("The flow step is invalid.", instance, steps);
        };
        if step.admissible.is_empty() {
            return self.complete_flow_transition(input, flow, instance, step, steps);
        }

        let capability = &step.admissible[0];
        let tools = self.registry.lookup_tool_by_capability(capability);
        steps.push(TurnTraceStep {
            name: "Registry.LookupTool".into(),
            detail: format!("{} => {} tools", capability, tools.len()),
        });
        let Some(tool) = tools.first().cloned().cloned() else {
            return failed_turn(
                "No registered tool satisfies this flow step.",
                instance,
                steps,
            );
        };
        instance
            .pinned_tool_versions
            .insert(tool.id.clone(), tool.version.clone());

        let mut frame = ExecutionFrame {
            slots: instance
                .slots
                .iter()
                .map(|(name, value)| (name.clone(), crate::ops::pure::json_to_value(value)))
                .collect(),
            state: Some(input.state_id.clone()),
            ..Default::default()
        };
        collect_user_params(&tool, &input.utterance, &mut frame.slots);
        steps.push(TurnTraceStep {
            name: "Policy.Wrap".into(),
            detail: capability.clone(),
        });
        let path = AbilityPath::seq([capability]);
        match self.execute(&path, frame, input, Some(&flow.id)) {
            Ok(PathExecution::NeedUser {
                frame,
                missing,
                question,
            }) => {
                update_flow_slots(instance, values_to_json(frame.slots));
                instance.pending_step = Some(step.id.clone());
                steps.push(TurnTraceStep {
                    name: "Bind.Residual".into(),
                    detail: missing.join(","),
                });
                suspended_turn(
                    Utterance {
                        text: question,
                        via: ExpressVia::Template,
                        template_id: Some(format!("{}_missing", step.id)),
                        claim_refs: vec![],
                        frame: None,
                    },
                    instance,
                    steps,
                )
            }
            Ok(PathExecution::Done {
                frame,
                invoked_tools,
            }) => {
                for tool_id in invoked_tools {
                    steps.push(TurnTraceStep {
                        name: "Invoke.Call".into(),
                        detail: format!("{tool_id} success"),
                    });
                }
                for trace in &frame.artifact_traces {
                    steps.push(TurnTraceStep {
                        name: "Artifact.Ledger".into(),
                        detail: trace.clone(),
                    });
                }
                let policy = PolicyCtx {
                    state: Some(input.state_id.clone()),
                    tenant: Some(self.tenant.tenant_id.clone()),
                    flow_id: Some(flow.id.clone()),
                    ..Default::default()
                };
                if !predicate_met(&step.postcondition, &frame, &policy) {
                    instance.attempts += 1;
                    steps.push(TurnTraceStep {
                        name: "Postcondition".into(),
                        detail: format!("{} failed", step.id),
                    });
                    return failed_turn(
                        "The flow step did not establish its postcondition.",
                        instance,
                        steps,
                    );
                }
                steps.push(TurnTraceStep {
                    name: "Postcondition".into(),
                    detail: format!("{} met", step.id),
                });
                update_flow_slots(instance, values_to_json(frame.slots));
                crate::blocks::flow::advance(instance, flow);
                if let Some(next) = flow.steps.get(instance.current_step_idx) {
                    if next.admissible.is_empty() {
                        self.complete_flow_transition(input, flow, instance, next, steps)
                    } else {
                        let next_tool = self
                            .registry
                            .lookup_tool_by_capability(&next.admissible[0])
                            .first()
                            .cloned()
                            .cloned();
                        let question = next_tool
                            .as_ref()
                            .and_then(|tool| {
                                tool.params.iter().find(|param| {
                                    param.required && !instance.slots.contains_key(&param.name)
                                })
                            })
                            .map(|param| express::ask(&param.name, param.prompt_hint.as_deref(), 1))
                            .unwrap_or_else(|| express::ask(&next.id, Some(&next.intent), 1));
                        suspended_turn(question, instance, steps)
                    }
                } else {
                    self.finish_flow(flow, instance, steps)
                }
            }
            Err(error) => {
                instance.attempts += 1;
                if let Some(trace) = &error.decision_trace {
                    steps.push(TurnTraceStep {
                        name: "Artifact.Ledger".into(),
                        detail: trace.to_string(),
                    });
                }
                steps.push(TurnTraceStep {
                    name: "Invoke.Error".into(),
                    detail: format!(
                        "reason={:?} recovery={:?}",
                        error.code,
                        error.code.recovery()
                    ),
                });
                failed_turn(&error.to_string(), instance, steps)
            }
        }
    }

    fn complete_flow_transition(
        &mut self,
        _input: &TurnInput,
        flow: &FlowSpec,
        instance: &mut FlowInstance,
        step: &crate::tenant::FlowStep,
        steps: &mut Vec<TurnTraceStep>,
    ) -> TurnResult {
        let Some(state) = flow.terminal_states.first().cloned() else {
            return failed_turn("The flow declares no terminal state.", instance, steps);
        };
        let frame = ExecutionFrame {
            state: Some(state.clone()),
            ..Default::default()
        };
        let policy = PolicyCtx {
            state: Some(state.clone()),
            transition: Some(state.clone()),
            flow_id: Some(flow.id.clone()),
            ..Default::default()
        };
        if !predicate_met(&step.postcondition, &frame, &policy) {
            return failed_turn(
                "The terminal transition postcondition failed.",
                instance,
                steps,
            );
        }
        steps.push(TurnTraceStep {
            name: "Postcondition".into(),
            detail: format!("{} met", step.id),
        });
        instance.pending_step = None;
        self.finish_flow_with_state(state, steps)
    }

    fn finish_flow(
        &self,
        flow: &FlowSpec,
        instance: &FlowInstance,
        steps: &[TurnTraceStep],
    ) -> TurnResult {
        match flow.terminal_states.first() {
            Some(state) => self.finish_flow_with_state(state.clone(), steps),
            None => failed_turn(
                "The flow completed without a terminal state.",
                instance,
                steps,
            ),
        }
    }

    fn finish_flow_with_state(&self, state: String, steps: &[TurnTraceStep]) -> TurnResult {
        let terminal_text = self
            .personality
            .and_then(|personality| personality.templates.get("flow_complete"))
            .map_or("Done.", String::as_str);
        TurnResult {
            reply: express::template("flow_complete", terminal_text, &IndexMap::new()),
            llm_calls: 0,
            tier: None,
            depth: Depth::Deep,
            steps: steps.to_vec(),
            opened_loop: false,
            new_state: Some(state),
            active_flow: None,
            situation_hash: None,
            proposal_id: None,
            suspended: false,
            graph_suspension: None,
        }
    }

    fn prepare_rank_query(
        &mut self,
        clause: &str,
        allow_model_extract: bool,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: &mut u32,
    ) -> Result<Option<(crate::blocks::term_resolve::QueryPlanFragment, String)>, Box<TurnResult>>
    {
        let rank = if let Some(rank) = crate::blocks::term_resolve::parse_rank_query(clause) {
            rank
        } else if allow_model_extract {
            let before = self.provider_llm_calls();
            let extracted = crate::blocks::term_resolve::parse_rank_query_with_provider(
                clause,
                self.llm_provider,
            );
            *llm_calls += self.provider_llm_calls().saturating_sub(before) as u32;
            match extracted {
                Ok((rank, prompt_hash)) => {
                    steps.push(TurnTraceStep {
                        name: "Understand.Rank".into(),
                        detail: format!("closed extraction prompt_hash={prompt_hash}"),
                    });
                    rank
                }
                Err(error) => {
                    steps.push(TurnTraceStep {
                        name: "Understand.Rank".into(),
                        detail: format!("failed closed: {:?}", error.code),
                    });
                    let mut result = rejected_path_turn(
                        steps.clone(),
                        "I recognized a declared ranking request but could not extract a safe query plan.",
                    );
                    result.llm_calls = *llm_calls;
                    return Err(Box::new(result));
                }
            }
        } else {
            return Ok(None);
        };
        let (term, limit, want_most) = rank;
        // Term resolution scores the term against *declared* attribute anchors, bridging novel words
        // semantically when a real embedder is present. The bag-of-hash default keeps it free and
        // deterministic; a gateway-backed embedder is what unlocks undeclared-synonym resolution.
        let res = crate::blocks::term_resolve::resolve_term(
            &term,
            want_most,
            limit,
            &self.tenant.attributes,
            0.15,
            self.embedder,
        );
        match res {
            crate::blocks::term_resolve::TermResolution::Confirm {
                attribute,
                margin,
                runners_up,
            } => {
                steps.push(TurnTraceStep {
                    name: "TermResolve".into(),
                    detail: format!("confirm {attribute} margin={margin}"),
                });
                let alternatives = runners_up
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let reply = express::clarify(&[format!(
                    "Should I interpret '{term}' as the declared attribute '{attribute}'? Candidates: {alternatives}. Reply yes or no."
                )]);
                Err(Box::new(TurnResult {
                    reply,
                    llm_calls: *llm_calls,
                    tier: None,
                    depth: Depth::Deep,
                    steps: steps.clone(),
                    opened_loop: true,
                    new_state: None,
                    active_flow: Some(term_confirmation_flow(&term, &attribute, limit, want_most)),
                    situation_hash: None,
                    proposal_id: None,
                    suspended: true,
                    graph_suspension: None,
                }))
            }
            crate::blocks::term_resolve::TermResolution::Resolved { plan, margin } => {
                steps.push(TurnTraceStep {
                    name: "TermResolve".into(),
                    detail: format!(
                        "{} {:?} limit={} margin={margin}",
                        plan.attribute, plan.direction, plan.limit
                    ),
                });
                let capability = self
                    .tenant
                    .attributes
                    .iter()
                    .find(|attribute| attribute.name == plan.attribute)
                    .and_then(|attribute| attribute.query_capability.clone())
                    .ok_or_else(|| {
                        rejected_path_turn(
                            steps.clone(),
                            "This sortable attribute has no declared query capability.",
                        )
                    })?;
                Ok(Some((plan, capability)))
            }
            crate::blocks::term_resolve::TermResolution::Unresolved => {
                let declared = self
                    .tenant
                    .attributes
                    .iter()
                    .filter(|attribute| attribute.sortable)
                    .map(|attribute| attribute.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let reply = express::clarify(&[format!(
                    "I couldn't safely map '{term}' without a semantic model. Use one of the declared sortable attributes: {declared}."
                )]);
                Err(Box::new(TurnResult {
                    reply,
                    llm_calls: *llm_calls,
                    tier: None,
                    depth: Depth::Deep,
                    steps: steps.clone(),
                    opened_loop: false,
                    new_state: None,
                    active_flow: None,
                    situation_hash: None,
                    proposal_id: None,
                    suspended: false,
                    graph_suspension: None,
                }))
            }
        }
    }

    fn execute_rank_plan(
        &mut self,
        input: &TurnInput,
        plan: &crate::blocks::term_resolve::QueryPlanFragment,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: &mut u32,
    ) -> TurnResult {
        let capability = self
            .tenant
            .attributes
            .iter()
            .find(|attribute| attribute.name == plan.attribute)
            .and_then(|attribute| attribute.query_capability.as_deref());
        let Some(capability) = capability else {
            return rejected_path_turn(
                steps.clone(),
                "This attribute has no declared query capability.",
            );
        };
        let mut planned_input = input.clone();
        planned_input.slots.insert(
            "attribute".into(),
            serde_json::Value::String(plan.attribute.clone()),
        );
        planned_input.slots.insert(
            "direction".into(),
            serde_json::Value::String(format!("{:?}", plan.direction).to_lowercase()),
        );
        planned_input
            .slots
            .insert("limit".into(), serde_json::Value::from(plan.limit));
        let state_node = state::read(self.states, &input.state_id).ok();
        let capabilities = state_node
            .as_ref()
            .map(|state| state.permission_envelope.clone())
            .unwrap_or_default();
        steps.push(TurnTraceStep {
            name: "Policy.Wrap".into(),
            detail: capability.into(),
        });
        let outcome = self.execute_tier_path(
            &planned_input,
            &AbilityPath::seq([capability]),
            &state_node,
            &capabilities,
            &[],
            steps,
        );
        *llm_calls = llm_calls.saturating_add(outcome.llm_calls);
        TurnResult {
            reply: outcome.reply,
            llm_calls: *llm_calls,
            tier: Some(LookupTier::Tier2),
            depth: Depth::Deep,
            steps: steps.clone(),
            opened_loop: outcome.opened_loop,
            new_state: None,
            active_flow: None,
            situation_hash: None,
            proposal_id: None,
            suspended: outcome.suspended,
            graph_suspension: None,
        }
    }

    /// Record the turn's outcome for σ and promote its path once the asymmetric gate clears. This
    /// is the seam that turns a one-off cold-path answer into a warm Tier-0/1 hit for the next time
    /// the same situation recurs. Effectful paths never auto-promote (tenant approval, decision P5).
    fn learn_from_turn(
        &mut self,
        sigma: &SituationKey,
        path: &AbilityPath,
        success: bool,
        steps: &mut Vec<TurnTraceStep>,
    ) -> String {
        let effectful = path_is_effectful(self.registry, path);
        let outcome = if self.inline_learning_enabled {
            observe_and_promote(
                self.proposals,
                self.registry,
                sigma,
                path.clone(),
                effectful,
                success,
                false,
                self.embedder,
            )
        } else {
            let proposal_id = crate::abilities::learn::observe(
                self.proposals,
                sigma,
                path.clone(),
                effectful,
                success,
            );
            ObserveOutcome::Accumulating {
                observations: self
                    .proposals
                    .get(&proposal_id)
                    .map_or(0, |proposal| proposal.observations),
            }
        };
        match outcome {
            ObserveOutcome::Promoted { procedure_id } => steps.push(TurnTraceStep {
                name: "Learn.Promote".into(),
                detail: format!("procedure={procedure_id} — σ warmed to Tier-0/1"),
            }),
            ObserveOutcome::Accumulating { observations } => steps.push(TurnTraceStep {
                name: "Learn.Observe".into(),
                detail: format!(
                    "obs={observations}/{PROMOTE_MIN_OBSERVATIONS} effectful={effectful}"
                ),
            }),
            ObserveOutcome::Blocked { reason } => steps.push(TurnTraceStep {
                name: "Learn.PromotionBlocked".into(),
                detail: reason,
            }),
        }
        crate::abilities::learn::proposal_id(sigma, path)
    }

    fn classify_declared_intent(
        &self,
        clause: &str,
        reachable_caps: &[String],
    ) -> Option<crate::abilities::understand::IntentClass> {
        (!reachable_caps.is_empty())
            .then(|| {
                crate::abilities::understand::classify_intent(clause, reachable_caps, self.embedder)
            })
            .flatten()
    }

    fn provider_llm_calls(&self) -> usize {
        self.llm_provider
            .calls()
            .iter()
            .filter(|call| matches!(call, ProviderCall::Llm { .. }))
            .count()
    }

    fn run_multi_clause(
        &mut self,
        input: &TurnInput,
        clauses: &[String],
        mut steps: Vec<TurnTraceStep>,
    ) -> TurnResult {
        const MAX_CLAUSES: usize = 4;
        if clauses.len() > MAX_CLAUSES {
            steps.push(TurnTraceStep {
                name: "MultiClauseGate".into(),
                detail: format!("bounded at {MAX_CLAUSES}; received {}", clauses.len()),
            });
            return multi_clause_suspension(express::clarify(&clauses[..MAX_CLAUSES]), steps);
        }
        if clauses_contradict(clauses) {
            steps.push(TurnTraceStep {
                name: "MultiClauseGate".into(),
                detail: "contradictory actions require clarification".into(),
            });
            return multi_clause_suspension(express::clarify(clauses), steps);
        }

        let risks: Vec<ClauseRisk> = clauses
            .iter()
            .map(|clause| self.classify_clause_risk(clause, input))
            .collect();
        let mut replies = Vec::new();
        let mut claim_refs = Vec::new();
        let mut llm_calls = 0;
        let mut new_state = None;
        // Reads are safe to run before asking permission for any mutation. Each recursive call is
        // a single-clause turn, so it cannot re-enter this coordinator.
        for (clause, risk) in clauses.iter().zip(&risks) {
            if *risk != ClauseRisk::ReadOnly {
                continue;
            }
            let mut sub_input = input.clone();
            sub_input.utterance = clause.clone();
            let result = self.run(&sub_input);
            llm_calls += result.llm_calls;
            claim_refs.extend(result.reply.claim_refs);
            replies.push(result.reply.text);
            steps.extend(result.steps.into_iter().map(|mut step| {
                step.name = format!("MultiClause.{}", step.name);
                step
            }));
            new_state = result.new_state.or(new_state);
            if result.suspended {
                return TurnResult {
                    reply: Utterance {
                        text: replies.join("\n"),
                        via: ExpressVia::Ask,
                        template_id: None,
                        claim_refs,
                        frame: None,
                    },
                    llm_calls,
                    tier: None,
                    depth: Depth::Deep,
                    steps,
                    opened_loop: true,
                    new_state,
                    active_flow: result.active_flow,
                    situation_hash: None,
                    proposal_id: None,
                    suspended: true,
                    graph_suspension: None,
                };
            }
        }

        let writes: Vec<String> = clauses
            .iter()
            .zip(risks)
            .filter(|(_, risk)| *risk != ClauseRisk::ReadOnly)
            .map(|(clause, _)| clause.clone())
            .collect();
        if !writes.is_empty() {
            let ordered = writes
                .iter()
                .enumerate()
                .map(|(index, clause)| format!("{}. {clause}", index + 1))
                .collect::<Vec<_>>()
                .join("; ");
            replies.push(express::confirm("execute these actions in order", &ordered).text);
            steps.push(TurnTraceStep {
                name: "MultiClauseGate".into(),
                detail: "reads completed; mutations withheld pending explicit confirmation".into(),
            });
            return TurnResult {
                reply: Utterance {
                    text: replies.join("\n"),
                    via: ExpressVia::Confirm,
                    template_id: None,
                    claim_refs,
                    frame: None,
                },
                llm_calls,
                tier: None,
                depth: Depth::Deep,
                steps,
                opened_loop: true,
                new_state,
                active_flow: Some(multi_clause_confirmation_flow(&writes)),
                situation_hash: None,
                proposal_id: None,
                suspended: true,
                graph_suspension: None,
            };
        }

        TurnResult {
            reply: Utterance {
                text: replies.join("\n"),
                via: ExpressVia::Synthesize,
                template_id: None,
                claim_refs,
                frame: None,
            },
            llm_calls,
            tier: None,
            depth: Depth::Deep,
            steps,
            opened_loop: false,
            new_state,
            active_flow: None,
            situation_hash: None,
            proposal_id: None,
            suspended: false,
            graph_suspension: None,
        }
    }

    fn classify_clause_risk(&self, clause: &str, input: &TurnInput) -> ClauseRisk {
        if self.matching_flow(clause, input).is_some() {
            return ClauseRisk::Mutation;
        }
        let words: HashSet<String> = clause
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        const MUTATION_WORDS: &[&str] = &[
            "send", "create", "update", "change", "cancel", "delete", "remove", "pay", "refund",
            "book", "schedule", "submit", "approve", "reject", "write",
        ];
        if MUTATION_WORDS.iter().any(|word| words.contains(*word)) {
            ClauseRisk::Mutation
        } else {
            ClauseRisk::ReadOnly
        }
    }

    fn resume_term_confirmation(
        &mut self,
        input: &TurnInput,
        steps: &mut Vec<TurnTraceStep>,
    ) -> TurnResult {
        let Some(flow) = input.active_flow.as_ref() else {
            return rejected_path_turn(steps.clone(), "The term confirmation state was missing.");
        };
        let term = flow.slots.get("term").and_then(serde_json::Value::as_str);
        let attribute = flow
            .slots
            .get("attribute")
            .and_then(serde_json::Value::as_str);
        let limit = flow.slots.get("limit").and_then(serde_json::Value::as_u64);
        let want_most = flow
            .slots
            .get("want_most")
            .and_then(serde_json::Value::as_bool);
        let (Some(term), Some(attribute), Some(limit), Some(want_most)) =
            (term, attribute, limit, want_most)
        else {
            return rejected_path_turn(
                steps.clone(),
                "The persisted term confirmation was incomplete.",
            );
        };
        let normalized = input.utterance.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "no" | "n" | "cancel" | "stop") {
            steps.push(TurnTraceStep {
                name: "TermConfirm".into(),
                detail: format!("rejected mapping {term} -> {attribute}"),
            });
            return multi_clause_completed("Okay, I won't use that interpretation.", steps.clone());
        }
        if !matches!(normalized.as_str(), "yes" | "y" | "confirm" | "proceed") {
            return TurnResult {
                reply: express::clarify(&[format!(
                    "Should I interpret '{term}' as '{attribute}'? Reply yes or no."
                )]),
                llm_calls: 0,
                tier: None,
                depth: Depth::Boundary,
                steps: steps.clone(),
                opened_loop: true,
                new_state: None,
                active_flow: input.active_flow.clone(),
                situation_hash: None,
                proposal_id: None,
                suspended: true,
                graph_suspension: None,
            };
        }
        let Some(plan) = crate::blocks::term_resolve::confirmed_plan(
            attribute,
            want_most,
            limit,
            &self.tenant.attributes,
        ) else {
            return rejected_path_turn(
                steps.clone(),
                "The confirmed attribute is no longer declared and sortable.",
            );
        };
        steps.push(TurnTraceStep {
            name: "TermConfirm".into(),
            detail: format!("accepted mapping {term} -> {attribute}"),
        });
        let mut llm_calls = 0;
        self.execute_rank_plan(input, &plan, steps, &mut llm_calls)
    }

    fn resume_multi_clause_plan(
        &mut self,
        input: &TurnInput,
        steps: &mut Vec<TurnTraceStep>,
    ) -> TurnResult {
        let plan = input
            .active_flow
            .as_ref()
            .and_then(|flow| flow.slots.get("clauses"))
            .and_then(serde_json::Value::as_array)
            .map(|clauses| {
                clauses
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let normalized = input.utterance.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "no" | "n" | "cancel" | "stop") {
            steps.push(TurnTraceStep {
                name: "MultiClauseConfirm".into(),
                detail: "ordered action plan cancelled".into(),
            });
            self.clear_pending_effect_confirmation();
            return multi_clause_completed("Okay, I cancelled that action plan.", steps.clone());
        }
        if !matches!(normalized.as_str(), "yes" | "y" | "confirm" | "proceed") {
            return TurnResult {
                reply: express::confirm("execute these actions in order", &plan.join("; ")),
                llm_calls: 0,
                tier: None,
                depth: Depth::Boundary,
                steps: steps.clone(),
                opened_loop: true,
                new_state: None,
                active_flow: input.active_flow.clone(),
                situation_hash: None,
                proposal_id: None,
                suspended: true,
                graph_suspension: None,
            };
        }

        steps.push(TurnTraceStep {
            name: "MultiClauseConfirm".into(),
            detail: format!("confirmed ordered plan with {} actions", plan.len()),
        });
        // Drop the harness pending marker before re-entering `run`. Confirmed execution carries
        // `__aelio_effect_confirmed` on each sub-turn; leaving the pending plan in place made
        // nested `run` calls treat the clause text as a fresh unconfirmed ask.
        self.clear_pending_effect_confirmation();
        let mut replies = Vec::new();
        let mut refs = Vec::new();
        let mut calls = 0;
        let mut state = None;
        for (index, clause) in plan.iter().cloned().enumerate() {
            let mut sub_input = input.clone();
            sub_input.utterance = clause;
            sub_input.active_flow = None;
            sub_input
                .slots
                .insert("__aelio_effect_confirmed".into(), serde_json::json!(true));
            let result = self.run(&sub_input);
            calls += result.llm_calls;
            refs.extend(result.reply.claim_refs);
            replies.push(result.reply.text);
            steps.extend(result.steps);
            state = result.new_state.or(state);
            if result.suspended {
                let mut active_flow = result.active_flow;
                if let Some(flow) = active_flow.as_mut() {
                    flow.slots
                        .insert("__aelio_plan_clauses".into(), serde_json::json!(plan));
                    flow.slots.insert(
                        "__aelio_plan_next_index".into(),
                        serde_json::json!(index + 1),
                    );
                }
                return TurnResult {
                    reply: Utterance {
                        text: replies.join("\n"),
                        via: result.reply.via,
                        template_id: result.reply.template_id,
                        claim_refs: refs,
                        frame: None,
                    },
                    llm_calls: calls,
                    tier: result.tier,
                    depth: Depth::Deep,
                    steps: steps.clone(),
                    opened_loop: true,
                    new_state: state,
                    active_flow,
                    situation_hash: result.situation_hash,
                    proposal_id: result.proposal_id,
                    suspended: true,
                    graph_suspension: None,
                };
            }
        }
        self.clear_pending_effect_confirmation();
        TurnResult {
            reply: Utterance {
                text: replies.join("\n"),
                via: ExpressVia::Synthesize,
                template_id: None,
                claim_refs: refs,
                frame: None,
            },
            llm_calls: calls,
            tier: None,
            depth: Depth::Deep,
            steps: steps.clone(),
            opened_loop: false,
            new_state: state,
            active_flow: None,
            situation_hash: None,
            proposal_id: None,
            suspended: false,
            graph_suspension: None,
        }
    }

    fn pending_effect_confirmation_clauses(&self) -> Option<Vec<String>> {
        self.harness
            .pages
            .get(crate::harness::CONDUCTOR_ID)
            .and_then(|page| page.slots.get(PENDING_EFFECT_CONFIRMATION_KEY))
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|clauses| !clauses.is_empty())
    }

    fn store_pending_effect_confirmation(&mut self, clauses: &[String]) {
        self.harness
            .ensure_page(crate::harness::CONDUCTOR_ID)
            .slots
            .insert(
                PENDING_EFFECT_CONFIRMATION_KEY.into(),
                serde_json::json!(clauses),
            );
    }

    fn clear_pending_effect_confirmation(&mut self) {
        if let Some(page) = self.harness.pages.get_mut(crate::harness::CONDUCTOR_ID) {
            page.slots.shift_remove(PENDING_EFFECT_CONFIRMATION_KEY);
        }
    }

    fn confirm_effectful_path(
        &mut self,
        input: &TurnInput,
        path: &AbilityPath,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: u32,
        tier: LookupTier,
    ) -> Option<TurnResult> {
        if !path_is_effectful(self.registry, path)
            || input
                .slots
                .get("__aelio_effect_confirmed")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        {
            return None;
        }
        let mut tools = Vec::new();
        for step in &path.steps {
            if let Some(tool) = self.registry.tools.get(&step.ability_id) {
                tools.push(tool.id.clone());
                continue;
            }
            for tool in self.registry.lookup_tool_by_capability(&step.ability_id) {
                tools.push(tool.id.clone());
            }
            if let Some(contract) = self.registry.abilities.get(&step.ability_id) {
                tools.extend(contract.tool_deps.iter().cloned());
            }
        }
        tools.sort();
        tools.dedup();
        let display_tools = tools
            .iter()
            .map(|tool_id| {
                self.registry
                    .tools
                    .get(tool_id)
                    .map(|tool| tool.name.as_str())
                    .unwrap_or(tool_id.as_str())
            })
            .collect::<Vec<_>>();
        let action = if display_tools.is_empty() {
            path.steps
                .iter()
                .map(|step| step.ability_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            display_tools.join(", ")
        };
        steps.push(TurnTraceStep {
            name: "EffectConfirm".into(),
            detail: format!(
                "withheld pending explicit confirmation: {action}; tool_ids={}",
                tools.join(",")
            ),
        });
        if !self.legacy_flow_execution_enabled {
            self.store_pending_effect_confirmation(std::slice::from_ref(&input.utterance));
        }
        Some(TurnResult {
            reply: express::confirm("execute this action", &action),
            llm_calls,
            tier: Some(tier),
            depth: Depth::Deep,
            steps: steps.clone(),
            opened_loop: true,
            new_state: None,
            active_flow: Some(multi_clause_confirmation_flow(std::slice::from_ref(
                &input.utterance,
            ))),
            situation_hash: None,
            proposal_id: None,
            suspended: true,
            graph_suspension: None,
        })
    }

    fn continue_confirmed_plan(
        &mut self,
        input: &TurnInput,
        clauses: Vec<String>,
        next_index: usize,
        completed: TurnResult,
    ) -> TurnResult {
        if next_index >= clauses.len() {
            return completed;
        }
        let remaining = clauses[next_index..].to_vec();
        let mut continuation_input = input.clone();
        continuation_input.utterance = "confirm".into();
        continuation_input.state_id = completed
            .new_state
            .clone()
            .unwrap_or_else(|| input.state_id.clone());
        continuation_input.active_flow = Some(multi_clause_confirmation_flow(&remaining));
        let mut continued =
            self.resume_multi_clause_plan(&continuation_input, &mut completed.steps.clone());
        if !completed.reply.text.is_empty() && !continued.reply.text.is_empty() {
            continued.reply.text = format!("{}\n{}", completed.reply.text, continued.reply.text);
        }
        continued.llm_calls = continued.llm_calls.saturating_add(completed.llm_calls);
        if continued.new_state.is_none() {
            continued.new_state = completed.new_state;
        }
        continued
    }

    fn synthesize_or_fallback(&mut self, evidence: &str) -> Utterance {
        let claim = crate::abilities::judge::EvidenceClaim {
            id: "turn.evidence.0".into(),
            value: Value::str(evidence),
            provenance: "verified_execution_frame".into(),
        };
        self.synthesize_claims_or_fallback(std::slice::from_ref(&claim))
    }

    fn synthesize_claims_or_fallback(
        &mut self,
        claims: &[crate::abilities::judge::EvidenceClaim],
    ) -> Utterance {
        express::synthesize_grounded_with_provider(claims, self.personality, &[], self.llm_provider)
            .unwrap_or_else(|_| {
                let evidence = claims
                    .iter()
                    .map(|claim| crate::ops::pure::value_to_json(&claim.value).to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut reply = express::synthesize(&evidence, self.personality, &[], None).0;
                reply
                    .claim_refs
                    .extend(claims.iter().map(|claim| claim.id.clone()));
                reply
            })
    }

    /// Conductor selects a starter harness. Returns `None` only for Escalate (cold ProposePath).
    fn run_conductor_starters(
        &mut self,
        input: &TurnInput,
        clause: &str,
        situation_hash: &str,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: u32,
    ) -> Option<TurnResult> {
        use crate::harness::{
            select_starter_harness, HarnessPlayMode, StarterHarness, CONDUCTOR_ID,
        };

        if self.harness.is_empty() {
            self.harness.push(CONDUCTOR_ID, false);
            steps.push(TurnTraceStep {
                name: "Conductor".into(),
                detail: "stack empty: pushed conductor as ceiling".into(),
            });
        }

        let choice = select_starter_harness(clause, self.harness);
        steps.push(TurnTraceStep {
            name: "Conductor.Select".into(),
            detail: format!("harness={} — {}", choice.id(), choice.description()),
        });

        match choice {
            StarterHarness::Escalate => None,
            other => {
                let harness_id = other.id();
                match self.harness_play_mode {
                    // `QuickReply` alone can decline its own answer (ReplyNumericGuard) and
                    // return `None`, falling through to escalation like `Escalate` itself would
                    // — it is not wrapped in `Some` here.
                    HarnessPlayMode::Hardcoded => match other {
                        StarterHarness::QuickReply => {
                            self.exec_quick_reply(input, clause, situation_hash, steps, llm_calls)
                        }
                        StarterHarness::UnderstandIntent => Some(self.exec_understand_intent(
                            input,
                            clause,
                            situation_hash,
                            steps,
                            llm_calls,
                        )),
                        StarterHarness::WaitForUser => Some(self.exec_wait_for_user(
                            input,
                            clause,
                            situation_hash,
                            steps,
                            llm_calls,
                        )),
                        StarterHarness::MemoryAttach => Some(self.exec_memory_attach(
                            input,
                            clause,
                            situation_hash,
                            steps,
                            llm_calls,
                        )),
                        StarterHarness::Escalate => unreachable!(),
                    },
                    HarnessPlayMode::Stored => Some(self.play_stored_starter(
                        harness_id,
                        input,
                        clause,
                        situation_hash,
                        steps,
                        llm_calls,
                    )),
                }
            }
        }
    }

    fn play_stored_starter(
        &mut self,
        harness_id: &str,
        input: &TurnInput,
        clause: &str,
        situation_hash: &str,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: u32,
    ) -> TurnResult {
        let Some(program) = self.tenant.harness_programs.get(harness_id).cloned() else {
            steps.push(TurnTraceStep {
                name: "Harness.Load".into(),
                detail: format!("missing program id={harness_id}"),
            });
            return rejected_path_turn(
                std::mem::take(steps),
                "The selected harness program is not installed in the catalog library.",
            );
        };
        if let Err(error) = program.validate() {
            steps.push(TurnTraceStep {
                name: "Harness.Load".into(),
                detail: format!("invalid program id={harness_id} reason={error}"),
            });
            return rejected_path_turn(
                std::mem::take(steps),
                "The installed harness program failed validation.",
            );
        }
        steps.push(TurnTraceStep {
            name: "Harness.Load".into(),
            detail: format!(
                "id={} version={} hash={}",
                program.id,
                program.version,
                program.content_hash()
            ),
        });
        self.play_harness_program(&program, input, clause, situation_hash, steps, llm_calls)
    }

    fn play_harness_program(
        &mut self,
        program: &crate::harness::HarnessProgramV1,
        input: &TurnInput,
        clause: &str,
        situation_hash: &str,
        steps: &mut Vec<TurnTraceStep>,
        mut llm_calls: u32,
    ) -> TurnResult {
        use crate::harness::{
            AttachSource, EvidenceSource, HarnessStepV1, QuerySource, CONDUCTOR_ID,
            WAIT_FOR_USER_ID,
        };

        let waiting = program
            .steps
            .iter()
            .any(|step| matches!(step, HarnessStepV1::AskAndWait { .. }));
        self.harness.push(&program.id, waiting);

        let mut reply: Option<Utterance> = None;
        let mut memory_snippet: Option<String> = None;
        let mut memory_claims: Vec<crate::abilities::judge::EvidenceClaim> = Vec::new();
        let mut opened_loop = false;
        let mut suspended = false;

        for step in &program.steps {
            match step {
                HarnessStepV1::PromptQuickReply { evidence_from } => {
                    steps.push(TurnTraceStep {
                        name: "Harness.Op".into(),
                        detail: format!("prompt.quick_reply evidence={evidence_from:?}"),
                    });
                    let notes = self
                        .harness
                        .pages
                        .get(CONDUCTOR_ID)
                        .map(|page| page.notes.join("; "))
                        .unwrap_or_default();
                    let is_tiny_greeting =
                        ["hi", "hello", "hey", "thanks", "thank you", "ok", "okay"]
                            .iter()
                            .any(|w| clause.trim().eq_ignore_ascii_case(w));
                    let (utterance, calls) = match evidence_from {
                        EvidenceSource::GreetingTemplateOrPageUser
                            if is_tiny_greeting && notes.is_empty() =>
                        {
                            (
                                express::greeting_template(
                                    self.personality,
                                    None,
                                    &[],
                                    false,
                                    false,
                                ),
                                0u32,
                            )
                        }
                        EvidenceSource::GreetingTemplateOrPageUser
                        | EvidenceSource::PageAndUser => {
                            let evidence = if !memory_claims.is_empty() {
                                None
                            } else if notes.is_empty() {
                                Some(format!(
                                    "User said: {clause}. Reply briefly in one or two sentences."
                                ))
                            } else {
                                Some(format!(
                                    "Context notes: {notes}\nUser said: {clause}. Reply briefly in one or two sentences."
                                ))
                            };
                            let before = self.provider_llm_calls();
                            let utterance = if !memory_claims.is_empty() {
                                self.synthesize_claims_or_fallback(&memory_claims)
                            } else {
                                self.synthesize_or_fallback(evidence.as_deref().unwrap_or(clause))
                            };
                            let calls = self.provider_llm_calls().saturating_sub(before) as u32;
                            (utterance, calls)
                        }
                        EvidenceSource::UserOnly => {
                            let evidence = format!(
                                "User said: {clause}. Reply briefly in one or two sentences."
                            );
                            let before = self.provider_llm_calls();
                            let utterance = self.synthesize_or_fallback(&evidence);
                            let calls = self.provider_llm_calls().saturating_sub(before) as u32;
                            (utterance, calls)
                        }
                    };
                    llm_calls = llm_calls.saturating_add(calls);
                    reply = Some(utterance);
                    self.harness.attach_note(
                        CONDUCTOR_ID,
                        format!("quick_reply handled: {}", truncate_for_note(clause, 80)),
                    );
                }
                HarnessStepV1::PromptUnderstandIntent => {
                    steps.push(TurnTraceStep {
                        name: "Harness.Op".into(),
                        detail: "prompt.understand_intent".into(),
                    });
                    let catalog = crate::harness::starter_catalog()
                        .iter()
                        .map(|(h, desc)| format!("{}: {desc}", h.id()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let evidence = format!(
                        "Classify the user's goal and suggest the next best starter harness.\n\
                         Available:\n{catalog}\n\
                         User: {clause}\n\
                         Reply in 1-3 short sentences: what they want, and which harness fits (or one clarifying question)."
                    );
                    let before = self.provider_llm_calls();
                    reply = Some(self.synthesize_or_fallback(&evidence));
                    llm_calls = llm_calls
                        .saturating_add(self.provider_llm_calls().saturating_sub(before) as u32);
                    self.harness.attach_note(
                        CONDUCTOR_ID,
                        format!("intent noted: {}", truncate_for_note(clause, 120)),
                    );
                }
                HarnessStepV1::MemorySearch { query_from } => {
                    steps.push(TurnTraceStep {
                        name: "Harness.Op".into(),
                        detail: format!("memory.search query={query_from:?}"),
                    });
                    let query = match query_from {
                        QuerySource::UserUtterance => clause,
                    };
                    let recalled = match self.turn_recall.retrieve(
                        &input.user_id,
                        query,
                        Utc::now().timestamp_millis(),
                        RecallBudget::default(),
                    ) {
                        Ok(recalled) => recalled,
                        Err(_) => Default::default(),
                    };
                    memory_claims = recalled.claims.clone();
                    memory_snippet = Some(if recalled.claims.is_empty() {
                        format!("no memory hit for: {}", truncate_for_note(query, 80))
                    } else {
                        recalled
                            .claims
                            .iter()
                            .take(3)
                            .map(|claim| crate::ops::pure::value_to_json(&claim.value).to_string())
                            .collect::<Vec<_>>()
                            .join(" | ")
                    });
                }
                HarnessStepV1::ContextAttach { from } => {
                    steps.push(TurnTraceStep {
                        name: "Harness.Op".into(),
                        detail: format!("context.attach from={from:?}"),
                    });
                    match from {
                        AttachSource::LastMemorySnippet => {
                            let snippet = memory_snippet
                                .clone()
                                .unwrap_or_else(|| "no memory snippet".into());
                            self.harness.attach_note(
                                CONDUCTOR_ID,
                                format!("memory: {}", truncate_for_note(&snippet, 200)),
                            );
                        }
                    }
                }
                HarnessStepV1::AskAndWait { question_template } => {
                    steps.push(TurnTraceStep {
                        name: "Harness.Op".into(),
                        detail: "ask_and_wait".into(),
                    });
                    steps.push(TurnTraceStep {
                        name: "Harness.wait_for_user".into(),
                        detail: "parked waiting for next user message".into(),
                    });
                    self.harness
                        .ensure_page(&program.id)
                        .slots
                        .insert("pending_prompt".into(), serde_json::json!(clause));
                    let truncated = truncate_for_note(clause, 60);
                    let question = if clause.contains('?') {
                        "Got it — what else should I know before we continue?".to_string()
                    } else {
                        question_template.replace("{clause}", &truncated)
                    };
                    self.harness
                        .attach_note(CONDUCTOR_ID, "wait_for_user parked");
                    reply = Some(express::ask("clarification", Some(&question), 1));
                    opened_loop = true;
                    suspended = true;
                    // Keep waiting frame; do not pop.
                    return TurnResult {
                        reply: reply.expect("ask_and_wait sets reply"),
                        llm_calls,
                        tier: Some(LookupTier::Tier3),
                        depth: Depth::Deep,
                        steps: std::mem::take(steps),
                        opened_loop,
                        new_state: None,
                        active_flow: None,
                        situation_hash: Some(situation_hash.into()),
                        proposal_id: None,
                        suspended,
                        graph_suspension: None,
                    };
                }
                HarnessStepV1::ReturnFinish | HarnessStepV1::ReturnParent => {
                    steps.push(TurnTraceStep {
                        name: "Harness.Op".into(),
                        detail: match step {
                            HarnessStepV1::ReturnFinish => "return.finish",
                            _ => "return.parent",
                        }
                        .into(),
                    });
                    let _ = self.harness.exit_up();
                    break;
                }
            }
        }

        // Compatibility step names for existing golden / harness tests.
        if program.id == crate::harness::QUICK_REPLY_ID {
            steps.push(TurnTraceStep {
                name: "Harness.quick_reply".into(),
                detail: "played from stored program".into(),
            });
        } else if program.id == crate::harness::UNDERSTAND_INTENT_ID {
            steps.push(TurnTraceStep {
                name: "Harness.understand_intent".into(),
                detail: "played from stored program".into(),
            });
        } else if program.id == crate::harness::MEMORY_ATTACH_ID {
            steps.push(TurnTraceStep {
                name: "Harness.memory_attach".into(),
                detail: "played from stored program".into(),
            });
        } else if program.id == WAIT_FOR_USER_ID {
            steps.push(TurnTraceStep {
                name: "Harness.wait_for_user".into(),
                detail: "played from stored program".into(),
            });
        }

        TurnResult {
            reply: reply
                .unwrap_or_else(|| express::template("harness_empty", "Done.", &IndexMap::new())),
            llm_calls,
            tier: Some(LookupTier::Tier3),
            depth: Depth::Deep,
            steps: std::mem::take(steps),
            opened_loop,
            new_state: None,
            active_flow: None,
            situation_hash: Some(situation_hash.into()),
            proposal_id: None,
            suspended,
            graph_suspension: None,
        }
    }

    /// Returns `None` when the synthesized reply fails `ReplyNumericGuard` (see F-032) —
    /// the caller falls through to escalation exactly as it does for `StarterHarness::Escalate`,
    /// rather than serving a conversational reply that states an unverified number as fact.
    fn exec_quick_reply(
        &mut self,
        input: &TurnInput,
        clause: &str,
        situation_hash: &str,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: u32,
    ) -> Option<TurnResult> {
        use crate::harness::{CONDUCTOR_ID, QUICK_REPLY_ID};

        self.harness.push(QUICK_REPLY_ID, false);
        let notes = self
            .harness
            .pages
            .get(CONDUCTOR_ID)
            .map(|page| page.notes.join("; "))
            .unwrap_or_default();
        steps.push(TurnTraceStep {
            name: "Harness.quick_reply".into(),
            detail: format!("notes_len={}", notes.len()),
        });

        // Short greetings stay model-free via authored template (Conductor still selected).
        let is_tiny_greeting = ["hi", "hello", "hey", "thanks", "thank you", "ok", "okay"]
            .iter()
            .any(|w| clause.trim().eq_ignore_ascii_case(w));

        let (reply, calls, synthesized) = if is_tiny_greeting && notes.is_empty() {
            (
                express::greeting_template(self.personality, None, &[], false, false),
                0u32,
                false,
            )
        } else {
            let evidence = if notes.is_empty() {
                format!("User said: {clause}. Reply briefly in one or two sentences.")
            } else {
                format!(
                    "Context notes: {notes}\nUser said: {clause}. Reply briefly in one or two sentences."
                )
            };
            let before = self.provider_llm_calls();
            let reply = self.synthesize_or_fallback(&evidence);
            let calls = self.provider_llm_calls().saturating_sub(before) as u32;
            (reply, calls, true)
        };

        // ReplyNumericGuard (F-032): a QuickReply never has a tool result behind it, so a
        // synthesized reply stating a number/currency/percent has nothing verifying it. High
        // recall by design — a false positive costs one re-triage through the cold path; a
        // false negative states a fabricated number as settled fact.
        if synthesized && reply_states_unverified_number(&reply.text) {
            steps.push(TurnTraceStep {
                name: "ReplyNumericGuard".into(),
                detail: "rejected: synthesized reply states an unverified number/currency/percent — escalating".into(),
            });
            let _ = self.harness.exit_up();
            return None;
        }

        self.harness.attach_note(
            CONDUCTOR_ID,
            format!("quick_reply handled: {}", truncate_for_note(clause, 80)),
        );
        let _ = self.harness.exit_up(); // pop quick_reply; conductor remains
        let _ = input;
        Some(TurnResult {
            reply,
            llm_calls: llm_calls.saturating_add(calls),
            tier: Some(LookupTier::Tier3),
            depth: Depth::Deep,
            steps: std::mem::take(steps),
            opened_loop: false,
            new_state: None,
            active_flow: None,
            situation_hash: Some(situation_hash.into()),
            proposal_id: None,
            suspended: false,
            graph_suspension: None,
        })
    }

    fn exec_understand_intent(
        &mut self,
        input: &TurnInput,
        clause: &str,
        situation_hash: &str,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: u32,
    ) -> TurnResult {
        use crate::harness::{CONDUCTOR_ID, UNDERSTAND_INTENT_ID};

        self.harness.push(UNDERSTAND_INTENT_ID, false);
        let catalog = crate::harness::starter_catalog()
            .iter()
            .map(|(h, desc)| format!("{}: {desc}", h.id()))
            .collect::<Vec<_>>()
            .join("\n");
        let evidence = format!(
            "Classify the user's goal and suggest the next best starter harness.\n\
             Available:\n{catalog}\n\
             User: {clause}\n\
             Reply in 1-3 short sentences: what they want, and which harness fits (or one clarifying question)."
        );
        steps.push(TurnTraceStep {
            name: "Harness.understand_intent".into(),
            detail: "classify + suggest next harness".into(),
        });
        let before = self.provider_llm_calls();
        let reply = self.synthesize_or_fallback(&evidence);
        let calls = self.provider_llm_calls().saturating_sub(before) as u32;
        self.harness.attach_note(
            CONDUCTOR_ID,
            format!("intent noted: {}", truncate_for_note(clause, 120)),
        );
        let _ = self.harness.exit_up();
        let _ = input;
        TurnResult {
            reply,
            llm_calls: llm_calls.saturating_add(calls),
            tier: Some(LookupTier::Tier3),
            depth: Depth::Deep,
            steps: std::mem::take(steps),
            opened_loop: false,
            new_state: None,
            active_flow: None,
            situation_hash: Some(situation_hash.into()),
            proposal_id: None,
            suspended: false,
            graph_suspension: None,
        }
    }

    fn exec_wait_for_user(
        &mut self,
        _input: &TurnInput,
        clause: &str,
        situation_hash: &str,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: u32,
    ) -> TurnResult {
        use crate::harness::{CONDUCTOR_ID, WAIT_FOR_USER_ID};

        self.harness.push(WAIT_FOR_USER_ID, true);
        self.harness
            .ensure_page(WAIT_FOR_USER_ID)
            .slots
            .insert("pending_prompt".into(), serde_json::json!(clause));
        let question = if clause.contains('?') {
            "Got it — what else should I know before we continue?".to_string()
        } else {
            format!(
                "Before I continue with “{}”, what should I clarify first?",
                truncate_for_note(clause, 60)
            )
        };
        steps.push(TurnTraceStep {
            name: "Harness.wait_for_user".into(),
            detail: "parked waiting for next user message".into(),
        });
        self.harness
            .attach_note(CONDUCTOR_ID, "wait_for_user parked");
        TurnResult {
            reply: express::ask("clarification", Some(&question), 1),
            llm_calls,
            tier: Some(LookupTier::Tier3),
            depth: Depth::Deep,
            steps: std::mem::take(steps),
            opened_loop: true,
            new_state: None,
            active_flow: None,
            situation_hash: Some(situation_hash.into()),
            proposal_id: None,
            suspended: true,
            graph_suspension: None,
        }
    }

    fn resume_wait_for_user(
        &mut self,
        input: &TurnInput,
        mut steps: Vec<TurnTraceStep>,
    ) -> TurnResult {
        use crate::harness::{CONDUCTOR_ID, WAIT_FOR_USER_ID};

        let prior = self
            .harness
            .pages
            .get(WAIT_FOR_USER_ID)
            .and_then(|page| page.slots.get("pending_prompt"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        steps.push(TurnTraceStep {
            name: "Harness.wait_for_user.resume".into(),
            detail: format!(
                "prior={} answer={}",
                truncate_for_note(&prior, 40),
                truncate_for_note(&input.utterance, 40)
            ),
        });
        self.harness.attach_note(
            CONDUCTOR_ID,
            format!(
                "user answered wait: {}",
                truncate_for_note(&input.utterance, 120)
            ),
        );
        // Clear waiting frame.
        if let Some(top) = self.harness.top_mut() {
            if top.harness_id == WAIT_FOR_USER_ID {
                top.waiting = false;
            }
        }
        let _ = self.harness.exit_up();
        let evidence = if prior.is_empty() {
            format!(
                "User answered a clarifying wait: {}. Acknowledge briefly and say next step.",
                input.utterance
            )
        } else {
            format!(
                "Earlier context: {prior}\nUser clarified: {}\nAcknowledge briefly and propose the next step.",
                input.utterance
            )
        };
        let before = self.provider_llm_calls();
        let reply = self.synthesize_or_fallback(&evidence);
        let calls = self.provider_llm_calls().saturating_sub(before) as u32;
        TurnResult {
            reply,
            llm_calls: calls,
            tier: Some(LookupTier::Tier3),
            depth: Depth::Deep,
            steps,
            opened_loop: false,
            new_state: None,
            active_flow: None,
            situation_hash: None,
            proposal_id: None,
            suspended: false,
            graph_suspension: None,
        }
    }

    fn exec_memory_attach(
        &mut self,
        input: &TurnInput,
        clause: &str,
        situation_hash: &str,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: u32,
    ) -> TurnResult {
        use crate::harness::{CONDUCTOR_ID, MEMORY_ATTACH_ID};

        self.harness.push(MEMORY_ATTACH_ID, false);
        let recalled = match self.turn_recall.retrieve(
            &input.user_id,
            clause,
            chrono::Utc::now().timestamp_millis(),
            RecallBudget::default(),
        ) {
            Ok(recalled) => recalled,
            Err(_) => Default::default(),
        };
        let snippet = if recalled.claims.is_empty() {
            format!("no memory hit for: {}", truncate_for_note(clause, 80))
        } else {
            recalled
                .claims
                .iter()
                .take(3)
                .map(|claim| crate::ops::pure::value_to_json(&claim.value).to_string())
                .collect::<Vec<_>>()
                .join(" | ")
        };
        self.harness.attach_note(
            CONDUCTOR_ID,
            format!("memory: {}", truncate_for_note(&snippet, 200)),
        );
        steps.push(TurnTraceStep {
            name: "Harness.memory_attach".into(),
            detail: format!("claims={}", recalled.claims.len()),
        });
        let evidence = if recalled.claims.is_empty() {
            format!(
                "No stored memory matched. User asked about: {clause}. Say you don't have that yet."
            )
        } else {
            format!("Attached memory snippets:\n{snippet}\nAnswer the user briefly grounded only in these.")
        };
        let before = self.provider_llm_calls();
        let reply = if recalled.claims.is_empty() {
            self.synthesize_or_fallback(&evidence)
        } else {
            self.synthesize_claims_or_fallback(&recalled.claims)
        };
        let calls = self.provider_llm_calls().saturating_sub(before) as u32;
        let _ = self.harness.exit_up();
        TurnResult {
            reply,
            llm_calls: llm_calls.saturating_add(calls),
            tier: Some(LookupTier::Tier3),
            depth: Depth::Deep,
            steps: std::mem::take(steps),
            opened_loop: false,
            new_state: None,
            active_flow: None,
            situation_hash: Some(situation_hash.into()),
            proposal_id: None,
            suspended: false,
            graph_suspension: None,
        }
    }
}

fn truncate_for_note(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn adaptive_output_utterance(
    output: crate::adaptive::AdaptiveArtifactOutputV1,
) -> Option<Utterance> {
    match (output.text, output.frame) {
        (Some(text), None) => Some(Utterance::plain(text, ExpressVia::Template)),
        (None, Some(frame)) => Some(Utterance {
            text: aelio_render::flatten_to_text(&frame),
            via: ExpressVia::Template,
            template_id: None,
            claim_refs: vec![],
            frame: Some(frame),
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClauseRisk {
    ReadOnly,
    Mutation,
}

fn clauses_contradict(clauses: &[String]) -> bool {
    let text = clauses.join(" ").to_ascii_lowercase();
    [
        ("cancel", "keep"),
        ("delete", "retain"),
        ("approve", "reject"),
    ]
    .iter()
    .any(|(left, right)| text.contains(left) && text.contains(right))
}

fn multi_clause_suspension(reply: Utterance, steps: Vec<TurnTraceStep>) -> TurnResult {
    TurnResult {
        reply,
        llm_calls: 0,
        tier: None,
        depth: Depth::Boundary,
        steps,
        opened_loop: true,
        new_state: None,
        active_flow: None,
        situation_hash: None,
        proposal_id: None,
        suspended: true,
        graph_suspension: None,
    }
}

fn multi_clause_confirmation_flow(clauses: &[String]) -> FlowInstance {
    FlowInstance {
        flow_id: "__aelio.multi_clause_confirmation".into(),
        flow_version: "1".into(),
        current_step_idx: 0,
        slots: indexmap::indexmap! {
            "clauses".into() => serde_json::json!(clauses),
        },
        attempts: 0,
        pinned_tool_versions: IndexMap::new(),
        pinned_prompt_hashes: IndexMap::new(),
        ttl_secs: Some(300),
        pending_step: Some("confirm".into()),
    }
}

fn term_confirmation_flow(
    term: &str,
    attribute: &str,
    limit: u64,
    want_most: bool,
) -> FlowInstance {
    FlowInstance {
        flow_id: "__aelio.term_confirmation".into(),
        flow_version: "1".into(),
        current_step_idx: 0,
        slots: indexmap::indexmap! {
            "term".into() => serde_json::json!(term),
            "attribute".into() => serde_json::json!(attribute),
            "limit".into() => serde_json::json!(limit),
            "want_most".into() => serde_json::json!(want_most),
        },
        attempts: 0,
        pinned_tool_versions: IndexMap::new(),
        pinned_prompt_hashes: IndexMap::new(),
        ttl_secs: Some(300),
        pending_step: Some("confirm".into()),
    }
}

fn pending_action_continuation(instance: &FlowInstance) -> Option<(Vec<String>, usize)> {
    let clauses = instance
        .slots
        .get("__aelio_plan_clauses")?
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let next = instance
        .slots
        .get("__aelio_plan_next_index")?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())?;
    Some((clauses, next))
}

fn multi_clause_completed(text: &str, steps: Vec<TurnTraceStep>) -> TurnResult {
    TurnResult {
        reply: Utterance {
            text: text.into(),
            via: ExpressVia::Template,
            template_id: Some("multi_clause_cancelled".into()),
            claim_refs: vec![],
            frame: None,
        },
        llm_calls: 0,
        tier: None,
        depth: Depth::Boundary,
        steps,
        opened_loop: false,
        new_state: None,
        active_flow: None,
        situation_hash: None,
        proposal_id: None,
        suspended: false,
        graph_suspension: None,
    }
}

fn collect_user_params(
    tool: &crate::tenant::ToolSpec,
    utterance: &str,
    slots: &mut IndexMap<String, Value>,
) {
    let missing: Vec<_> = tool
        .params
        .iter()
        .filter(|param| {
            param.required
                && (matches!(param.source, crate::tenant::ParamSource::User)
                    || (matches!(param.source, crate::tenant::ParamSource::Slot { .. })
                        && !matches!(param.sensitivity, crate::types::Sensitivity::None)))
                && !slots.contains_key(&param.name)
        })
        .collect();
    for param in &missing {
        let extracted = if param.name.eq_ignore_ascii_case("phone") {
            crate::abilities::understand::extract_phone(utterance)
        } else {
            match &param.constraint {
                Some(ParamConstraint::Pattern { regex }) => {
                    regex::Regex::new(regex).ok().and_then(|pattern| {
                        utterance
                            .split_whitespace()
                            .map(|token| token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric()))
                            .find(|token| pattern.is_match(token))
                            .map(str::to_owned)
                    })
                }
                Some(ParamConstraint::Format { kind }) if kind == "e164" => {
                    crate::abilities::understand::extract_phone(utterance)
                }
                _ if missing.len() == 1 && !utterance.chars().any(char::is_whitespace) => {
                    Some(utterance.to_string())
                }
                _ => None,
            }
        };
        if let Some(value) = extracted {
            slots.insert(param.name.clone(), Value::str(value));
        }
    }
}

fn values_to_json(values: IndexMap<String, Value>) -> IndexMap<String, serde_json::Value> {
    values
        .into_iter()
        .map(|(name, value)| (name, crate::ops::pure::value_to_json(&value)))
        .collect()
}

fn update_flow_slots(
    instance: &mut FlowInstance,
    mut business_slots: IndexMap<String, serde_json::Value>,
) {
    for (name, value) in &instance.slots {
        if name.starts_with("__aelio_") {
            business_slots.insert(name.clone(), value.clone());
        }
    }
    instance.slots = business_slots;
}

fn suspended_turn(
    reply: Utterance,
    instance: &FlowInstance,
    steps: &[TurnTraceStep],
) -> TurnResult {
    TurnResult {
        reply,
        llm_calls: 0,
        tier: None,
        depth: Depth::Deep,
        steps: steps.to_vec(),
        opened_loop: true,
        new_state: None,
        active_flow: Some(instance.clone()),
        situation_hash: None,
        proposal_id: None,
        suspended: true,
        graph_suspension: None,
    }
}

fn failed_turn(message: &str, instance: &FlowInstance, steps: &[TurnTraceStep]) -> TurnResult {
    suspended_turn(
        express::apologize(message, "Please try again."),
        instance,
        steps,
    )
}

fn deferred_runtime_flow_turn(steps: Vec<TurnTraceStep>) -> TurnResult {
    TurnResult {
        reply: express::template(
            "flow_intent_deferred",
            "I’m finishing your current task first. I’ve saved this request and will return to it next.",
            &IndexMap::new(),
        ),
        llm_calls: 0,
        tier: None,
        depth: Depth::Deep,
        steps,
        opened_loop: true,
        new_state: None,
        active_flow: None,
        situation_hash: None,
        proposal_id: None,
        suspended: true,
                graph_suspension: None,
    }
}

/// Conservative lexical guard for the deterministic multi-clause gate: tells an independently
/// actionable clause ("cancel my order") from a bare noun conjunct ("invoices") that the crude
/// marker splitter produced. Real clause detection is the LLM `Understand.SplitClauses` ability's
/// job; this only keeps the gate from firing on ordinary conjunctions. Self-contained by design so
/// it does not collide with other splitter guards under concurrent edit.
/// True when `text` contains a digit or a currency/percent symbol. Used by `ReplyNumericGuard`
/// (F-032) to keep a QuickReply from stating an unverified number as fact — deliberately blunt:
/// it does not try to distinguish "the 3rd time" from "$3,000 refund," because the cost of a
/// false positive (one extra re-triage) is far cheaper than the cost of a false negative (a
/// confidently fabricated number reaching the user with no tool result behind it).
fn reply_states_unverified_number(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_ascii_digit() || matches!(c, '$' | '€' | '£' | '¥' | '%'))
}

fn clause_looks_actionable(clause: &str) -> bool {
    const REQUEST_WORDS: &[&str] = &[
        "show", "list", "find", "get", "check", "tell", "send", "resend", "create", "update",
        "change", "cancel", "delete", "remove", "add", "pay", "refund", "book", "schedule",
        "verify", "help", "want", "need", "please", "login", "logout", "log",
    ];
    clause
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .any(|word| REQUEST_WORDS.contains(&word.as_str()))
}

fn intent_class(utterance: &str) -> String {
    if crate::blocks::term_resolve::parse_rank_query(utterance).is_some() {
        return "rank_query".into();
    }
    let d = classify_depth(utterance);
    match d.depth {
        Depth::Shallow => "greeting".into(),
        Depth::Boundary => "state_query".into(),
        Depth::Deep => "general_deep".into(),
    }
}

fn goal_spec(
    registry: &Registry,
    intent: &str,
    capabilities: &[String],
    input: &TurnInput,
) -> crate::abilities::learn::GoalSpec {
    let mut produces = Vec::new();
    for tool in registry.lookup_tool_by_capability(intent) {
        for (name, field) in &tool.output_semantics.fields {
            produces.push(name.clone());
            produces.push(field.path.clone());
        }
    }
    if produces.is_empty() {
        produces.push(intent.to_string());
    }
    produces.sort();
    produces.dedup();
    crate::abilities::learn::GoalSpec {
        intent: intent.into(),
        produces,
        available_slots: input.slots.keys().cloned().collect(),
        allowed_capabilities: capabilities.to_vec(),
        allow_effects: false,
    }
}

fn caps_labels(caps: &[String]) -> Vec<String> {
    caps.iter()
        .map(|capability| capability.replace(['.', '_'], " "))
        .collect()
}

fn rejected_path_turn(steps: Vec<TurnTraceStep>, message: &str) -> TurnResult {
    TurnResult {
        reply: express::apologize(message, "Please try a simpler request."),
        llm_calls: 0,
        tier: None,
        depth: Depth::Deep,
        steps,
        opened_loop: false,
        new_state: None,
        active_flow: None,
        situation_hash: None,
        proposal_id: None,
        suspended: false,
        graph_suspension: None,
    }
}

/// Decision logs may explain closed structural mismatches because these messages contain only
/// declared field names, paths and type tags. Other error messages can originate in a tenant tool
/// and therefore stay hidden from the redacted trace.
fn trace_safe_error_detail(error: &AelioError) -> String {
    // Conflict/Unavailable messages are structural (instance pins, host disconnect) and contain no
    // tenant PII — surface them so operators can diagnose tool-proxy identity bugs. SigMismatch
    // messages name declared field paths only.
    if matches!(
        error.code,
        crate::types::ReasonCode::SigMismatch
            | crate::types::ReasonCode::Conflict
            | crate::types::ReasonCode::Unavailable
    ) {
        format!("failed: {:?}: {}", error.code, error.message)
    } else {
        format!("failed: {:?}", error.code)
    }
}

fn record_adaptive_shadow(
    tier: &crate::abilities::learn::TierLookup,
    situation_hash: &str,
    input: &TurnInput,
    steps: &mut Vec<TurnTraceStep>,
) -> Option<crate::adaptive::AdaptiveDecisionEnvelopeV1> {
    let projected = serde_json::json!({
        "turn": {
            "utterance": input.utterance,
            "channel": input.channel,
            "state_id": input.state_id,
        }
    });
    match crate::adaptive::from_tier_lookup(tier, situation_hash, projected) {
        Ok(decision) => {
            let (selected, parity, reason) = match &decision.selected {
                crate::adaptive::AdaptiveDecisionV1::Invoke { artifact, .. } => (
                    format!("invoke {}", artifact.key()),
                    true,
                    "exact_artifact_pin",
                ),
                crate::adaptive::AdaptiveDecisionV1::Insufficient { .. } => (
                    "insufficient".into(),
                    matches!(tier.tier, LookupTier::Tier3),
                    if tier.tier == LookupTier::Tier3 {
                        "both_miss"
                    } else {
                        "legacy_path_not_admitted"
                    },
                ),
                crate::adaptive::AdaptiveDecisionV1::Abstain { .. } => (
                    "abstain".into(),
                    false,
                    "legacy_procedure_lacks_unified_artifact",
                ),
                crate::adaptive::AdaptiveDecisionV1::Reply { .. } => {
                    ("reply".into(), false, "unexpected_reply_at_tier_seam")
                }
            };
            steps.push(TurnTraceStep {
                name: "AdaptiveDecision.Shadow".into(),
                detail: format!(
                    "selected={selected} decision_hash={}",
                    decision.decision_hash
                ),
            });
            steps.push(TurnTraceStep {
                name: "AdaptiveParity".into(),
                detail: format!("match={parity} reason={reason}"),
            });
            Some(decision)
        }
        Err(error) => {
            steps.push(TurnTraceStep {
                name: "AdaptiveDecision.Shadow".into(),
                detail: format!("invalid reason={:?}", error.code),
            });
            steps.push(TurnTraceStep {
                name: "AdaptiveParity".into(),
                detail: "match=false reason=invalid_closed_decision".into(),
            });
            None
        }
    }
}

/// Cold-loop score stub.
pub fn cold_score_turn(
    tool_ok: bool,
    repair: RepairSignal,
    flow_terminal: bool,
    abandoned: bool,
    latency_ms: u64,
    token_cost: u64,
) -> crate::abilities::judge::OutcomeScore {
    crate::abilities::judge::outcome(&crate::abilities::judge::OutcomeSignals {
        tool_non_error: tool_ok,
        repair_detected: matches!(repair, RepairSignal::Repair),
        flow_terminal,
        abandoned,
        latency_ms,
        token_cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actionable_count(utterance: &str) -> usize {
        let (_, clauses) = split_clauses(utterance);
        clauses
            .iter()
            .filter(|c| clause_looks_actionable(c))
            .count()
    }

    #[test]
    fn noun_conjunction_does_not_gate() {
        // Splits into ["show my orders", "invoices"] but only one segment is a request, so the
        // multi-clause gate (which fires only at 2+ actionable clauses) must not fire — the user
        // asked one clear thing.
        assert!(actionable_count("show my orders and invoices") < 2);
        assert!(actionable_count("what is my name and email") < 2);
    }

    #[test]
    fn two_real_requests_gate() {
        assert!(actionable_count("cancel my order and send me a receipt") > 1);
        assert!(actionable_count("show my invoices and update my address") > 1);
    }

    #[test]
    fn bare_noun_is_not_actionable() {
        assert!(!clause_looks_actionable("invoices"));
        assert!(!clause_looks_actionable("my account"));
        assert!(clause_looks_actionable("cancel my order"));
    }

    #[test]
    fn reply_numeric_guard_flags_digits_and_currency_and_percent() {
        assert!(reply_states_unverified_number(
            "Your balance is 42 dollars."
        ));
        assert!(reply_states_unverified_number("That'll be $19.99."));
        assert!(reply_states_unverified_number("Roughly 15% off today."));
        assert!(!reply_states_unverified_number(
            "Sure, I can help with that — happy to look into it."
        ));
    }
}
