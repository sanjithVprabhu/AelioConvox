//! Turn spine — the hot loop.
//!
//! sense → flow gate → clause split → triage → situation key → tier lookup
//! → execute → synthesize → write back

use crate::abilities::express::{self, ExpressVia, Utterance};
use crate::abilities::invoke::ToolHost;
use crate::abilities::learn::{
    lookup_tier_with_embedder, observe_and_promote, path_is_effectful, propose_path_greeting,
    propose_path_with_provider, situation_hash, situation_key, typecheck, ObserveOutcome,
    ProposalMap, SituationKey, PROMOTE_MIN_OBSERVATIONS,
};
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
    pub utterance: String,
    pub user_id: String,
    pub channel: String,
    pub turn_index: u64,
    pub last_seen_secs_ago: Option<u64>,
    pub state_id: String,
    pub slots: IndexMap<String, serde_json::Value>,
    pub active_flow: Option<FlowInstance>,
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
    pub tool_host: &'a mut dyn ToolHost,
    pub llm_provider: &'a mut dyn LlmProvider,
    pub turn_recall: &'a mut dyn TurnRecall,
    /// The runtime's configured embedder — bag-of-hash offline, or the gateway-served OpenAI /
    /// Anthropic / Gemini model in production. Every semantic hot-path decision (Tier-1 σ near-match,
    /// intent classification, term bridging, promotion embedding) goes through this one handle.
    pub embedder: &'a dyn crate::embedding::Embedder,
    pub inline_learning_enabled: bool,
    pub(crate) durable_memory_enabled: bool,
}

impl<'a> TurnRuntime<'a> {
    pub fn run(&mut self, input: &TurnInput) -> TurnResult {
        let mut steps = Vec::new();
        let mut llm_calls = 0u32;

        if input
            .active_flow
            .as_ref()
            .is_some_and(|flow| flow.flow_id == "__aelio.multi_clause_confirmation")
        {
            return self.resume_multi_clause_plan(input, &mut steps);
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

        // Authored flow triggers outrank semantic depth. A trigger such as "log in" may be
        // lexically shallow, but it is still a control-rail activation and must not fall through
        // to greeting/banter handling.
        if let Some(flow) = self.matching_flow(clause, input) {
            steps.push(TurnTraceStep {
                name: "FlowMatch".into(),
                detail: format!("matched {} before semantic triage", flow.id),
            });
            let mut result = self.activate_flow(input, &flow, &mut steps);
            result.llm_calls = result.llm_calls.saturating_add(llm_calls);
            return result;
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

        match depth_class.depth {
            Depth::Shallow => {
                let mut result = self.shallow_turn(input, &session, &state_node, &mut steps, &env);
                result.llm_calls = result.llm_calls.saturating_add(llm_calls);
                return result;
            }
            Depth::Boundary => {
                return self.boundary_turn(input, &mut steps, &mut llm_calls);
            }
            Depth::Deep => {}
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
        steps.push(TurnTraceStep {
            name: "LookupTier".into(),
            detail: format!("{:?}", tier.tier),
        });

        match tier.tier {
            LookupTier::Tier0 | LookupTier::Tier1 => {
                let procedure_id = tier.procedure_id.clone();
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
                }
            }
            LookupTier::Tier3 => {
                // LLM proposes path over declared abilities
                let before = self.provider_llm_calls();
                let ability_ids = self.registry.abilities.keys().cloned().collect::<Vec<_>>();
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
                steps.push(TurnTraceStep {
                    name: "ProposePath".into(),
                    detail: format!(
                        "{} steps prompt_hash={proposal_prompt_hash}",
                        path.steps.len()
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
            let procedure_id = tier.procedure_id.clone();
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
            };
        }

        // Greeting structure is authored and deterministic; only output-oriented banter needs
        // synthesis intelligence. Do not pay a model to rediscover the built-in safe greeting path.
        let path = propose_path_greeting();
        let mut llm_calls = 0;
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
        }
    }

    fn boundary_turn(
        &mut self,
        input: &TurnInput,
        steps: &mut Vec<TurnTraceStep>,
        llm_calls: &mut u32,
    ) -> TurnResult {
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
                        detail: trace.clone(),
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
                steps.push(TurnTraceStep {
                    name: "Execute".into(),
                    detail: format!("failed: {:?}", error.code),
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
            };
        }

        steps.push(TurnTraceStep {
            name: "MultiClauseConfirm".into(),
            detail: format!("confirmed ordered plan with {} actions", plan.len()),
        });
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
                };
            }
        }
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
        }
    }

    fn confirm_effectful_path(
        &self,
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
        let action = if tools.is_empty() {
            path.steps
                .iter()
                .map(|step| step.ability_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            tools.join(", ")
        };
        steps.push(TurnTraceStep {
            name: "EffectConfirm".into(),
            detail: format!("withheld pending explicit confirmation: {action}"),
        });
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
    }
}

fn failed_turn(message: &str, instance: &FlowInstance, steps: &[TurnTraceStep]) -> TurnResult {
    suspended_turn(
        express::apologize(message, "Please try again."),
        instance,
        steps,
    )
}

/// Conservative lexical guard for the deterministic multi-clause gate: tells an independently
/// actionable clause ("cancel my order") from a bare noun conjunct ("invoices") that the crude
/// marker splitter produced. Real clause detection is the LLM `Understand.SplitClauses` ability's
/// job; this only keeps the gate from firing on ordinary conjunctions. Self-contained by design so
/// it does not collide with other splitter guards under concurrent edit.
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
}
