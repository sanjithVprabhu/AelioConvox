//! In-memory tenant world for tests and local bake.

use crate::abilities::invoke::{MockToolHost, ToolHost};
use crate::abilities::learn::{default_situation_embedder, ProposalMap};
use crate::abilities::registry::Registry;
use crate::abilities::sig::SignatureRegistry;
use crate::blocks::turn::{ShadowPathOutcome, TurnInput, TurnResult, TurnRuntime};
use crate::contract::AbilityPath;
use crate::contract::Predicate;
use crate::embedding::Embedder;
use crate::ops::effects::EffectEnv;
use crate::provider::{LlmProvider, ProviderCall, ScriptedLlmProvider, UnavailableLlmProvider};
use crate::runtime::turn_recall::{NoopTurnRecall, TurnRecall};
use crate::tenant::*;
use crate::types::{Sensitivity, TenantMode};
use indexmap::IndexMap;
use std::collections::HashSet;

pub struct World {
    pub tenant: TenantDecl,
    pub registry: Registry,
    pub effect_env: EffectEnv,
    pub proposals: ProposalMap,
    pub signatures: SignatureRegistry,
    pub once_seen: HashSet<String>,
    pub tool_host: Box<dyn ToolHost>,
    pub adaptive_artifact_host: Box<dyn crate::adaptive::AdaptiveArtifactHost>,
    pub user_state: IndexMap<String, String>,
    pub user_flows: IndexMap<String, crate::blocks::flow::FlowInstance>,
    pub llm_provider: Box<dyn LlmProvider>,
    pub turn_recall: Box<dyn TurnRecall>,
    /// The embedder every semantic hot-path decision uses. Defaults to the offline bag-of-hash model;
    /// swap in `GatewayEmbedder` to route embeddings through the TS gateway (OpenAI/Anthropic/Gemini).
    pub embedder: std::sync::Arc<dyn Embedder>,
    /// Local bake may warm immediately; durable production uses behavioral signals + cold loop.
    pub inline_learning_enabled: bool,
    /// Transitional local/parity switch. Unified production constructors disable this so an
    /// unmaterialized FlowSpec cannot regain effect or continuation authority.
    pub legacy_flow_execution_enabled: bool,
    /// True only when a wrapper can actually commit user-scoped memory durably.
    pub(crate) durable_memory_enabled: bool,
}

impl World {
    /// Production-safe bootstrap with no vertical catalog, mock behavior, or synthetic model
    /// output. A validated durable/SDK catalog must be installed before serving useful turns.
    pub fn empty_tenant(tenant_id: &str) -> crate::types::AelioResult<Self> {
        let mut world = Self::demo_tenant(tenant_id);
        world.replace_tenant(TenantDecl::empty(tenant_id))?;
        world.tool_host = Box::new(MockToolHost::default());
        world.adaptive_artifact_host = Box::new(crate::adaptive::UnavailableAdaptiveArtifactHost);
        world.llm_provider = Box::new(UnavailableLlmProvider::default());
        world.inline_learning_enabled = false;
        world.legacy_flow_execution_enabled = false;
        Ok(world)
    }

    /// Atomically replace the active tenant declaration while preserving durable/in-flight
    /// user state. Entity versions remain immutable in durable storage; this method rebuilds
    /// only the active in-memory lookup surface.
    pub fn replace_tenant(&mut self, tenant: TenantDecl) -> crate::types::AelioResult<()> {
        if tenant.tenant_id != self.tenant.tenant_id {
            return Err(crate::types::AelioError::new(
                crate::types::ReasonCode::Denied,
                "cannot replace a runtime with another tenant's catalog",
            ));
        }
        let mut registry = Registry::default();
        for tool in &tenant.tools {
            registry.register_tool(tool.clone());
        }
        register_tool_capabilities(&mut registry);
        for flow in &tenant.flows {
            registry.register_flow(flow.clone());
        }
        for (flow_id, artifact) in &tenant.flow_artifacts {
            registry.bind_flow_artifact(flow_id, artifact.clone())?;
        }
        for id in [
            "State.Direction",
            "Registry.Capabilities",
            "Express.Template",
            "Express.Synthesize",
            "Express.Ask",
            "Learn.ProposePath",
        ] {
            registry.register_ability(builtin_contract(id));
        }
        self.tenant = tenant;
        self.registry = registry;
        Ok(())
    }

    pub fn demo_tenant(tenant_id: &str) -> Self {
        let mut tenant = TenantDecl::empty(tenant_id);
        tenant.mode = TenantMode::Live;

        tenant.states = vec![
            StateSpec {
                id: "unauthenticated".into(),
                name: "Unauthenticated".into(),
                permission_envelope: vec!["auth.otp.send".into(), "catalog.browse".into()],
                direction: Some(StateDirection {
                    target: "authenticated".into(),
                    nudge_policy: "soft".into(),
                }),
                entry_conditions: vec![],
                exit_edges: vec![ExitEdge {
                    to: "authenticated".into(),
                    guard: Predicate::True,
                    evidence_required: true,
                }],
                timeout: None,
            },
            StateSpec {
                id: "authenticated".into(),
                name: "Authenticated".into(),
                permission_envelope: vec![
                    "catalog.browse".into(),
                    "crm.clients.query".into(),
                    "orders.list".into(),
                ],
                direction: None,
                entry_conditions: vec![],
                exit_edges: vec![],
                timeout: None,
            },
        ];

        tenant.personalities = vec![PersonalitySpec {
            id: "default".into(),
            voice: VoiceSpec {
                register: "friendly".into(),
                verbosity: "medium".into(),
                formality: "casual".into(),
                emoji_policy: "sparse".into(),
            },
            lexicon: LexiconSpec::default(),
            constraints: vec!["never promise a delivery date".into()],
            templates: indexmap::indexmap! {
                "greeting_unauth".into() =>
                    "Hey! I can help you {capabilities}.".into(),
            },
        }];

        tenant.tools = vec![
            ToolSpec {
                id: "send_otp".into(),
                name: "send_otp".into(),
                version: "1".into(),
                capability_tags: vec!["auth.otp.send".into()],
                effect: Some(crate::tenant::ToolEffect::External),
                effectful: true,
                idempotent: false,
                dry_run_available: true,
                params: vec![
                    ParamSpec {
                        name: "tenant_id".into(),
                        type_name: "str".into(),
                        required: true,
                        constraint: None,
                        source: ParamSource::Const {
                            value: crate::types::Value::str(tenant_id),
                        },
                        repair: None,
                        prompt_hint: None,
                        sensitivity: Sensitivity::None,
                        default: None,
                        depends_on: vec![],
                    },
                    ParamSpec {
                        name: "phone".into(),
                        type_name: "str".into(),
                        required: true,
                        constraint: Some(ParamConstraint::Format {
                            kind: "e164".into(),
                        }),
                        source: ParamSource::User,
                        repair: Some(RepairFn::NormalizePhone {
                            region: "IN".into(),
                        }),
                        prompt_hint: Some("What number should I text the code to?".into()),
                        sensitivity: Sensitivity::Pii,
                        default: None,
                        depends_on: vec![],
                    },
                ],
                output_semantics: OutputSpec {
                    fields: IndexMap::new(),
                    role_hint: Some("effect_confirmation".into()),
                },
                continuations: vec!["auth.otp.verify".into()],
                errors: vec![],
            },
            ToolSpec {
                id: "verify_otp".into(),
                name: "verify_otp".into(),
                version: "1".into(),
                capability_tags: vec!["auth.otp.verify".into()],
                effect: Some(crate::tenant::ToolEffect::External),
                effectful: true,
                idempotent: false,
                dry_run_available: false,
                params: vec![
                    ParamSpec {
                        name: "phone".into(),
                        type_name: "str".into(),
                        required: true,
                        constraint: None,
                        source: ParamSource::Slot {
                            name: "phone".into(),
                        },
                        repair: None,
                        prompt_hint: None,
                        sensitivity: Sensitivity::Pii,
                        default: None,
                        depends_on: vec![],
                    },
                    ParamSpec {
                        name: "otp".into(),
                        type_name: "str".into(),
                        required: true,
                        constraint: Some(ParamConstraint::Pattern {
                            regex: r"^\d{6}$".into(),
                        }),
                        source: ParamSource::User,
                        repair: None,
                        prompt_hint: Some("What's the 6-digit code?".into()),
                        sensitivity: Sensitivity::Secret,
                        default: None,
                        depends_on: vec![],
                    },
                ],
                output_semantics: OutputSpec {
                    fields: indexmap::indexmap! {
                        "otp_verified".into() => OutputField {
                            path: "ok".into(),
                            type_name: "bool".into(),
                            sensitivity: Sensitivity::None,
                            meaning: "the submitted one-time code was verified".into(),
                        },
                    },
                    role_hint: Some("effect_confirmation".into()),
                },
                continuations: vec![],
                errors: vec![ErrorSpec {
                    match_code: "invalid_otp".into(),
                    reason: "needs_repair".into(),
                    recovery: "re-ask".into(),
                }],
            },
            ToolSpec {
                id: "clients_query".into(),
                name: "clients_query".into(),
                version: "1".into(),
                capability_tags: vec!["crm.clients.query".into()],
                effect: Some(crate::tenant::ToolEffect::Read),
                effectful: false,
                idempotent: true,
                dry_run_available: true,
                params: vec![
                    ParamSpec {
                        name: "sort_by".into(),
                        type_name: "str".into(),
                        required: true,
                        constraint: None,
                        source: ParamSource::Derived {
                            expr: "slot:attribute".into(),
                        },
                        repair: None,
                        prompt_hint: None,
                        sensitivity: Sensitivity::None,
                        default: None,
                        depends_on: vec![],
                    },
                    ParamSpec {
                        name: "order".into(),
                        type_name: "str".into(),
                        required: true,
                        constraint: None,
                        source: ParamSource::Derived {
                            expr: "slot:direction".into(),
                        },
                        repair: None,
                        prompt_hint: None,
                        sensitivity: Sensitivity::None,
                        default: None,
                        depends_on: vec![],
                    },
                    ParamSpec {
                        name: "limit".into(),
                        type_name: "int".into(),
                        required: true,
                        constraint: None,
                        source: ParamSource::Derived {
                            expr: "slot:limit".into(),
                        },
                        repair: None,
                        prompt_hint: None,
                        sensitivity: Sensitivity::None,
                        default: Some(crate::types::Value::Int(10)),
                        depends_on: vec![],
                    },
                ],
                output_semantics: OutputSpec {
                    fields: indexmap::indexmap! {
                        "clients".into() => OutputField {
                            path: "clients".into(),
                            type_name: "list".into(),
                            sensitivity: Sensitivity::None,
                            meaning: "the bounded client records returned by the declared sort plan".into(),
                        },
                    },
                    role_hint: Some("data".into()),
                },
                continuations: vec![],
                errors: vec![],
            },
        ];

        tenant.flows = vec![FlowSpec {
            id: "login".into(),
            version: "1".into(),
            name: "Login with OTP".into(),
            activation: FlowActivation {
                hard_preconditions: vec![Predicate::state_eq("unauthenticated")],
                trigger_surface: vec!["login".into(), "sign in".into(), "log in".into()],
                margin_threshold: 0.7,
            },
            learnable: false,
            preemption: Preemption::Hold,
            steps: vec![
                FlowStep {
                    id: "collect_phone".into(),
                    intent: "collect phone".into(),
                    postcondition: Predicate::Present {
                        path: "slot.phone".into(),
                    },
                    admissible: vec!["auth.otp.send".into()],
                    on_violation: ViolationAction::Repair,
                    suspendable: true,
                },
                FlowStep {
                    id: "await_otp".into(),
                    intent: "verify otp".into(),
                    postcondition: Predicate::Present {
                        path: "evidence.otp_verified".into(),
                    },
                    admissible: vec!["auth.otp.verify".into()],
                    on_violation: ViolationAction::Repair,
                    suspendable: true,
                },
                FlowStep {
                    id: "transition".into(),
                    intent: "authenticate".into(),
                    postcondition: Predicate::state_eq("authenticated"),
                    admissible: vec![],
                    on_violation: ViolationAction::Escape,
                    suspendable: false,
                },
            ],
            escape: FlowEscape::Escalate,
            terminal_states: vec!["authenticated".into()],
            ttl_secs: Some(300),
            max_attempts: 3,
            lowering: None,
        }];

        tenant.attributes = vec![
            AttributeSpec {
                name: "discount_pressure_score".into(),
                anchors: vec![
                    "greedy".into(),
                    "haggler".into(),
                    "price-sensitive".into(),
                    "always negotiating".into(),
                ],
                // "avaricious" is a tenant/learn-loop declared synonym — vertical knowledge lives in
                // the declaration, never in the engine. With a real embedder the bag-of-hash default
                // could also bridge it semantically; declaring it keeps the demo model-free.
                learned: vec!["avaricious".into()],
                polarity: Polarity::HighMeansMore,
                sortable: true,
                query_capability: Some("crm.clients.query".into()),
            },
            AttributeSpec {
                name: "generosity_index".into(),
                anchors: vec!["generous".into(), "giving".into()],
                learned: vec![],
                polarity: Polarity::HighMeansMore,
                sortable: true,
                query_capability: Some("crm.clients.query".into()),
            },
        ];

        tenant.policies = vec![PolicySpec {
            id: "allow-all-demo".into(),
            effect: PolicyEffect::Allow,
            subject: PolicySubject::default(),
            action: PolicyAction::default(),
            condition: Predicate::True,
            reason_code: "ok".into(),
            priority: 1,
        }];

        let mut registry = Registry::default();
        for t in &tenant.tools {
            registry.register_tool(t.clone());
        }
        register_tool_capabilities(&mut registry);
        for f in &tenant.flows {
            registry.register_flow(f.clone());
        }
        for id in [
            "State.Direction",
            "Registry.Capabilities",
            "Express.Template",
            "Express.Synthesize",
            "Express.Ask",
            "Learn.ProposePath",
        ] {
            registry.register_ability(builtin_contract(id));
        }

        let tool_host = MockToolHost::default()
            .on("send_otp", |_args| {
                Ok(crate::types::Value::Map(indexmap::indexmap! {
                    "ok".into() => crate::types::Value::Bool(true),
                    "continuation".into() => crate::types::Value::str("auth.otp.verify"),
                }))
            })
            .on("verify_otp", |args| {
                if args.get("otp").and_then(|value| value.as_str()) == Some("434543") {
                    Ok(crate::types::Value::Map(indexmap::indexmap! {
                        "ok".into() => crate::types::Value::Bool(true),
                    }))
                } else {
                    Ok(crate::types::Value::Map(indexmap::indexmap! {
                        "error".into() => crate::types::Value::str("invalid_otp"),
                    }))
                }
            })
            .on("clients_query", |_args| {
                // Sample rows so the demo proves tool-call → *grounded* answer (the reply is built
                // from these payload rows via claim_refs, not a hardcoded template). Real tenants
                // return their own data through the SDK bridge; this is only the demo host.
                let row = |name: &str, score: f64| {
                    crate::types::Value::Map(indexmap::indexmap! {
                        "name".into() => crate::types::Value::str(name),
                        "discount_pressure_score".into() => crate::types::Value::Float(score),
                    })
                };
                Ok(crate::types::Value::Map(indexmap::indexmap! {
                    "clients".into() => crate::types::Value::List(vec![
                        row("Acme Corp", 0.94),
                        row("Globex", 0.88),
                        row("Initech", 0.81),
                    ]),
                }))
            });

        Self {
            tenant,
            registry,
            effect_env: EffectEnv::live(),
            proposals: IndexMap::new(),
            signatures: SignatureRegistry::default(),
            once_seen: HashSet::new(),
            tool_host: Box::new(tool_host),
            adaptive_artifact_host: Box::new(crate::adaptive::UnavailableAdaptiveArtifactHost),
            user_state: IndexMap::new(),
            user_flows: IndexMap::new(),
            llm_provider: Box::new(ScriptedLlmProvider::deterministic()),
            turn_recall: Box::new(NoopTurnRecall),
            embedder: std::sync::Arc::new(default_situation_embedder()),
            inline_learning_enabled: true,
            legacy_flow_execution_enabled: true,
            durable_memory_enabled: false,
        }
    }

    /// Route embeddings through a real model (e.g. `GatewayEmbedder` → TS gateway). Must be a stable
    /// choice per deployment: σ embeddings stored by promotion live in this embedder's space, so a
    /// query embedder from a different space would never near-match them.
    pub fn set_embedder(&mut self, embedder: Box<dyn Embedder>) {
        self.embedder = std::sync::Arc::from(embedder);
    }

    pub fn set_shared_embedder(&mut self, embedder: std::sync::Arc<dyn Embedder>) {
        self.embedder = embedder;
    }

    pub fn set_llm_provider(&mut self, provider: Box<dyn LlmProvider>) {
        self.llm_provider = provider;
    }

    pub fn set_tool_host(&mut self, host: Box<dyn ToolHost>) {
        self.tool_host = host;
    }

    pub fn set_adaptive_artifact_host(
        &mut self,
        host: Box<dyn crate::adaptive::AdaptiveArtifactHost>,
    ) {
        self.adaptive_artifact_host = host;
    }

    pub fn disable_legacy_flow_execution(&mut self) {
        self.legacy_flow_execution_enabled = false;
    }

    pub fn provider_calls(&self) -> &[ProviderCall] {
        self.llm_provider.calls()
    }

    pub fn run_turn(&mut self, user_id: &str, utterance: &str) -> TurnResult {
        self.run_turn_on_channel(user_id, utterance, "unknown")
    }

    pub fn run_turn_on_channel(
        &mut self,
        user_id: &str,
        utterance: &str,
        channel: &str,
    ) -> TurnResult {
        let local_turn_id = format!("local-turn-{}", self.effect_env.ledger.len());
        self.run_turn_on_channel_with_id(user_id, utterance, channel, &local_turn_id)
    }

    pub fn run_turn_on_channel_with_id(
        &mut self,
        user_id: &str,
        utterance: &str,
        channel: &str,
        turn_id: &str,
    ) -> TurnResult {
        let state_id = self
            .user_state
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| "unauthenticated".into());
        let turn_index = self
            .effect_env
            .ledger
            .iter()
            .filter(|r| r.kind == "turn_user" && r.payload.as_str() == Some(user_id))
            .count() as u64;
        let _ = self
            .effect_env
            .ledger_append("turn_user", crate::types::Value::str(user_id));

        let active_flow = self.user_flows.get(user_id).cloned();
        let input = TurnInput {
            turn_id: turn_id.into(),
            utterance: utterance.into(),
            user_id: user_id.into(),
            channel: channel.into(),
            turn_index,
            last_seen_secs_ago: if turn_index == 0 { None } else { Some(60) },
            state_id,
            slots: IndexMap::new(),
            active_flow,
        };

        let policies = self.tenant.policies.clone();
        let states = self.tenant.states.clone();
        let personality = self.tenant.personalities.first().cloned();

        let mut rt = TurnRuntime {
            tenant: &self.tenant,
            registry: &mut self.registry,
            policies: &policies,
            states: &states,
            personality: personality.as_ref(),
            effect_env: &mut self.effect_env,
            proposals: &mut self.proposals,
            signatures: &mut self.signatures,
            once_seen: &mut self.once_seen,
            tool_host: self.tool_host.as_mut(),
            adaptive_artifact_host: self.adaptive_artifact_host.as_mut(),
            llm_provider: self.llm_provider.as_mut(),
            turn_recall: self.turn_recall.as_mut(),
            embedder: self.embedder.as_ref(),
            inline_learning_enabled: self.inline_learning_enabled,
            legacy_flow_execution_enabled: self.legacy_flow_execution_enabled,
            durable_memory_enabled: self.durable_memory_enabled,
        };
        let result = rt.run(&input);

        if let Some(ref st) = result.new_state {
            self.user_state.insert(user_id.into(), st.clone());
        }
        if self.legacy_flow_execution_enabled {
            match &result.active_flow {
                Some(flow) => {
                    self.user_flows.insert(user_id.into(), flow.clone());
                }
                None if !result.suspended => {
                    self.user_flows.shift_remove(user_id);
                }
                None => {}
            }
        }

        // Emit only after the in-memory state/flow commit. DurableRuntime adds a separate durable
        // commit line after its final compare-and-swap succeeds.
        if crate::decision_log::enabled() {
            eprintln!(
                "{}",
                crate::decision_log::render(
                    input.turn_index,
                    &input.user_id,
                    &input.utterance,
                    &input.state_id,
                    &result,
                )
            );
        }

        result
    }

    pub(crate) fn run_shadow_path(
        &mut self,
        user_id: &str,
        utterance: &str,
        path: &AbilityPath,
    ) -> crate::types::AelioResult<ShadowPathOutcome> {
        let state_id = self
            .user_state
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| "unauthenticated".into());
        let turn_index = self
            .effect_env
            .ledger
            .iter()
            .filter(|record| record.kind == "turn_user" && record.payload.as_str() == Some(user_id))
            .count() as u64;
        let input = TurnInput {
            turn_id: format!("shadow-turn-{turn_index}"),
            utterance: utterance.into(),
            user_id: user_id.into(),
            channel: "shadow".into(),
            turn_index,
            last_seen_secs_ago: None,
            state_id,
            slots: IndexMap::new(),
            active_flow: None,
        };
        let policies = self.tenant.policies.clone();
        let states = self.tenant.states.clone();
        let personality = self.tenant.personalities.first().cloned();
        TurnRuntime {
            tenant: &self.tenant,
            registry: &mut self.registry,
            policies: &policies,
            states: &states,
            personality: personality.as_ref(),
            effect_env: &mut self.effect_env,
            proposals: &mut self.proposals,
            signatures: &mut self.signatures,
            once_seen: &mut self.once_seen,
            tool_host: self.tool_host.as_mut(),
            adaptive_artifact_host: self.adaptive_artifact_host.as_mut(),
            llm_provider: self.llm_provider.as_mut(),
            turn_recall: self.turn_recall.as_mut(),
            embedder: self.embedder.as_ref(),
            inline_learning_enabled: false,
            legacy_flow_execution_enabled: self.legacy_flow_execution_enabled,
            durable_memory_enabled: self.durable_memory_enabled,
        }
        .run_shadow_path(&input, path)
    }
}

fn builtin_contract(id: &str) -> crate::contract::AbilityContract {
    let mut contract = crate::contract::AbilityContract::pure(id);
    contract.prompt_hash = match id {
        "Express.Synthesize" => crate::abilities::express::synthesize_prompt_spec()
            .canonical_hash()
            .ok(),
        "Learn.ProposePath" => crate::abilities::learn::propose_path_prompt_spec()
            .canonical_hash()
            .ok(),
        _ => None,
    };
    contract
}

fn register_tool_capabilities(registry: &mut Registry) {
    let capabilities = registry
        .by_capability
        .iter()
        .map(|(capability, tool_ids)| (capability.clone(), tool_ids.clone()))
        .collect::<Vec<_>>();
    for (capability, tool_ids) in capabilities {
        let effectful = tool_ids.iter().any(|tool_id| {
            registry
                .tools
                .get(tool_id)
                .is_some_and(|tool| tool.effectful)
        });
        let contract = if effectful {
            crate::contract::AbilityContract::effect(capability)
        } else {
            crate::contract::AbilityContract::pure(capability)
        }
        .with_tool_deps(tool_ids);
        registry.register_ability(contract);
    }
}

#[cfg(test)]
mod adaptive_execution_tests {
    use super::*;
    use crate::abilities::learn::{situation_hash, situation_key};
    use crate::abilities::registry::{
        ProcedureEvidence, ProcedureProvenance, ProcedureSpec, ProcedureStatus, SituationFilter,
    };
    use crate::adaptive::{
        AdaptiveArtifactHost, AdaptiveArtifactOutputV1, AdaptiveArtifactTurnV1,
        AdaptiveDecisionEnvelopeV1, ArtifactPinV1,
    };
    use std::sync::{Arc, Mutex};

    struct RecordingArtifactHost {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl AdaptiveArtifactHost for RecordingArtifactHost {
        fn invoke(
            &mut self,
            _tenant: &str,
            instance_id: &str,
            decision: &AdaptiveDecisionEnvelopeV1,
        ) -> crate::types::AelioResult<AdaptiveArtifactTurnV1> {
            decision.validate()?;
            self.calls.lock().unwrap().push(instance_id.into());
            Ok(AdaptiveArtifactTurnV1 {
                output: AdaptiveArtifactOutputV1 {
                    text: Some("Hello from unified runtime".into()),
                    frame: None,
                },
                suspended: false,
            })
        }
    }

    #[test]
    fn exact_bound_procedure_executes_via_artifact_host_inside_turn() {
        let mut world = World::demo_tenant("tenant-a");
        let capabilities = world.tenant.states[0].permission_envelope.clone();
        let sigma = situation_key(
            "unauthenticated",
            "greeting",
            vec![],
            capabilities.clone(),
            None,
            0,
            None,
        );
        let procedure_id = "procedure-greeting-v1";
        world.registry.register_procedure(ProcedureSpec {
            id: procedure_id.into(),
            version: "1".into(),
            tenant_id: "tenant-a".into(),
            situation_hash: situation_hash(&sigma),
            situation_filter: SituationFilter {
                state: Some("unauthenticated".into()),
                intent_class: Some("greeting".into()),
                required_slots: vec![],
                capability_tags: capabilities,
            },
            situation_embedding: vec![],
            path: AbilityPath::seq(["Express.Template"]),
            contract: world.registry.abilities["Express.Template"].clone(),
            tool_deps: vec![],
            prompt_deps: vec![],
            evidence: ProcedureEvidence {
                observations: 20,
                success_rate: 1.0,
                mean_cost: 0.0,
                mean_latency_ms: 1.0,
            },
            status: ProcedureStatus::Promoted,
            provenance: ProcedureProvenance {
                origin: "test".into(),
                proposed_by: "test".into(),
                approved_by: Some("test".into()),
            },
            supersedes: None,
        });
        world
            .registry
            .bind_procedure_artifact(
                procedure_id,
                ArtifactPinV1 {
                    id: "procedure.greeting".into(),
                    version: 1,
                    hash: "a".repeat(64),
                },
            )
            .unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        world.set_adaptive_artifact_host(Box::new(RecordingArtifactHost {
            calls: Arc::clone(&calls),
        }));

        let result = world.run_turn_on_channel_with_id("user-1", "hi", "web", "turn-exact-1");
        assert_eq!(result.reply.text, "Hello from unified runtime");
        assert_eq!(&*calls.lock().unwrap(), &["turn-exact-1"]);
        assert!(result.steps.iter().any(|step| {
            step.name == "AdaptiveInvoke" && step.detail.contains("authority=aelio-runtime")
        }));
    }
}
