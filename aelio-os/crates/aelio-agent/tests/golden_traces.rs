//! Golden traces from the architecture document.
//! V1 evaluation harness: expected step sequences + LLM-call assertions.

use aelio_agent::blocks::flow::FlowInstance;
use aelio_agent::contract::{AbilityContract, Predicate};
use aelio_agent::runtime::World;
use aelio_agent::types::{Depth, LookupTier};
use indexmap::IndexMap;

/// Greeting is model-free via Conductor → quick_reply template (no cold ProposePath).
#[test]
fn hi_cold_path_is_model_free_and_learnable() {
    let mut world = World::demo_tenant("t_demo");
    let r = world.run_turn("u1", "Hi");

    assert_eq!(r.depth, Depth::Shallow);
    assert_eq!(r.llm_calls, 0);
    assert!(!r.opened_loop, "bare greeting opens no loop");
    assert!(
        r.reply.text.to_lowercase().contains("help") || r.reply.text.to_lowercase().contains("hey")
    );
    assert!(r.steps.iter().any(|s| s.name == "Sense"));
    assert!(r.steps.iter().any(|s| s.name == "FlowGate"));
    assert!(r.steps.iter().any(|s| s.name == "SplitClauses"));
    assert!(r.steps.iter().any(|s| s.name == "ClassifyDepth"));
    assert!(r.steps.iter().any(|s| s.name == "Conductor.Select"));
    assert!(r.steps.iter().any(|s| s.name == "Harness.quick_reply"));
    assert!(!r.steps.iter().any(|s| s.name == "ProposePath"));
    // pre-gate skipped LLM split
    let split = r.steps.iter().find(|s| s.name == "SplitClauses").unwrap();
    assert!(split.detail.contains("mode=pure"));
    assert!(world.registry.procedures.is_empty());
}

/// Compatibility inline promotion is disabled; only the durable cold loop may promote.
#[test]
fn repeated_world_turn_does_not_inline_promote() {
    let mut world = World::demo_tenant("t_demo");
    let cold = world.run_turn("u1", "Hi");
    assert_eq!(cold.llm_calls, 0);
    assert!(world.registry.procedures.is_empty());

    let repeated = world.run_turn("u2", "Hi");
    assert_eq!(repeated.llm_calls, 0);
    assert!(repeated.steps.iter().any(|s| s.name == "Conductor.Select"));
    assert!(world.registry.procedures.is_empty());
}

/// Trace: "Hi" from user parked mid-OTP — flow context outranks semantics.
#[test]
fn hi_mid_flow_resumes_not_greets() {
    let mut world = World::demo_tenant("t_demo");
    world.user_flows.insert(
        "u1".into(),
        FlowInstance {
            flow_id: "login".into(),
            flow_version: "1".into(),
            current_step_idx: 1,
            slots: indexmap::indexmap! {
                "phone".into() => serde_json::json!("+919876543210"),
            },
            attempts: 0,
            pinned_tool_versions: IndexMap::new(),
            pinned_prompt_hashes: IndexMap::new(),
            ttl_secs: Some(300),
            pending_step: Some("await_otp".into()),
        },
    );

    let r = world.run_turn("u1", "Hi");
    assert_eq!(r.llm_calls, 0);
    assert!(r.suspended);
    assert!(
        r.reply.text.contains("6-digit") || r.reply.text.contains("code"),
        "should remind OTP, got: {}",
        r.reply.text
    );
    assert!(r
        .steps
        .iter()
        .any(|s| s.name == "FlowGate" && s.detail.contains("resume")));
    // triage should not be the path that greets
    assert!(!r.reply.text.to_lowercase().starts_with("hey!"));
}

/// Trace: login activation resolves tools by capability tags.
#[test]
fn login_resolves_tools_by_capability() {
    let mut world = World::demo_tenant("t_demo");
    let r = world.run_turn("u1", "I want to login");
    assert_eq!(r.depth, Depth::Deep);
    assert!(r.suspended);
    assert!(r.opened_loop);
    assert!(r.steps.iter().any(|s| s.name == "Registry.LookupTool"));
    assert!(
        r.reply.text.to_lowercase().contains("number")
            || r.reply.text.to_lowercase().contains("code"),
        "got: {}",
        r.reply.text
    );
    assert!(world.user_flows.contains_key("u1"));
}

#[test]
fn shallow_login_trigger_preempts_depth_triage() {
    let mut world = World::demo_tenant("t_demo");
    let result = world.run_turn("u1", "log in");

    assert!(result.suspended);
    assert_eq!(
        result
            .active_flow
            .as_ref()
            .map(|flow| flow.flow_id.as_str()),
        Some("login")
    );
    assert!(result.steps.iter().any(|step| step.name == "FlowMatch"));
    assert!(!result.steps.iter().any(|step| step.name == "ClassifyDepth"));
    assert_eq!(world.tool_host.invocation_count(), Some(0));
}

/// A provided phone executes the authored flow's send capability through ToolCallBlock.
#[test]
fn login_with_phone_actually_invokes_send_otp() {
    let mut world = World::demo_tenant("t_demo");
    let r = world.run_turn("u1", "login with +919876543210");

    assert!(r.suspended);
    assert_eq!(world.tool_host.invocation_count_for("send_otp"), Some(1));
    assert!(r
        .steps
        .iter()
        .any(|step| step.name == "Policy.Wrap" && step.detail == "auth.otp.send"));
    assert!(r
        .steps
        .iter()
        .any(|step| step.name == "Postcondition" && step.detail == "collect_phone met"));
}

/// Trace: OTP resume with code completes auth.
#[test]
fn login_otp_resume_authenticates() {
    let mut world = World::demo_tenant("t_demo");
    world.user_flows.insert(
        "u1".into(),
        FlowInstance {
            flow_id: "login".into(),
            flow_version: "1".into(),
            current_step_idx: 1,
            slots: indexmap::indexmap! {
                "phone".into() => serde_json::json!("+919876543210"),
            },
            attempts: 0,
            pinned_tool_versions: IndexMap::new(),
            pinned_prompt_hashes: IndexMap::new(),
            ttl_secs: Some(300),
            pending_step: Some("await_otp".into()),
        },
    );

    let r = world.run_turn("u1", "434543");
    assert_eq!(r.llm_calls, 0);
    assert_eq!(r.new_state.as_deref(), Some("authenticated"));
    assert!(r.reply.text.to_lowercase().contains("done"));
    assert_eq!(world.tool_host.invocation_count_for("verify_otp"), Some(1));
    assert!(r
        .steps
        .iter()
        .any(|step| step.name == "Postcondition" && step.detail == "await_otp met"));
}

#[test]
fn wrong_otp_is_typed_repair_not_signature_mismatch() {
    let mut world = World::demo_tenant("t_demo");
    let first = world.run_turn("u1", "login with +919876543210");
    assert!(first.suspended);

    let result = world.run_turn("u1", "111111");
    assert!(result.suspended);
    assert!(result.steps.iter().any(|step| {
        step.name == "Invoke.Error"
            && step.detail.contains("NeedsRepair")
            && !step.detail.contains("SigMismatch")
    }));
    assert_eq!(
        result
            .active_flow
            .as_ref()
            .and_then(|flow| flow.pending_step.as_deref()),
        Some("await_otp")
    );
}

/// Effect policy denial happens before the host call.
#[test]
fn flow_effect_is_policy_wrapped() {
    use aelio_agent::contract::Predicate;
    use aelio_agent::tenant::{PolicyAction, PolicyEffect, PolicySpec, PolicySubject};

    let mut world = World::demo_tenant("t_demo");
    world.tenant.policies.push(PolicySpec {
        id: "deny-otp-send".into(),
        effect: PolicyEffect::Deny,
        subject: PolicySubject::default(),
        action: PolicyAction {
            capability: Some("auth.otp.send".into()),
            ..Default::default()
        },
        condition: Predicate::True,
        reason_code: "otp_disabled".into(),
        priority: 100,
    });

    let r = world.run_turn("u1", "login with +919876543210");
    assert!(r.suspended);
    assert_eq!(world.tool_host.invocation_count_for("send_otp"), Some(0));
    assert!(r.reply.text.contains("PolicyDenied"));
}

/// An arbitrary registered capability path executes without turn-level specialization.
#[test]
fn arbitrary_capability_path_executes_generically() {
    use aelio_agent::blocks::executor::{
        execute_path, ExecutionFrame, PathExecution, PathExecutionContext,
    };
    use aelio_agent::contract::AbilityPath;
    use aelio_agent::policy::PolicyCtx;
    use aelio_agent::tenant::{OutputField, OutputSpec, ToolSpec};

    let mut world = World::demo_tenant("t_demo");
    world.registry.register_tool(ToolSpec {
        id: "custom_ping".into(),
        name: "custom_ping".into(),
        version: "1".into(),
        capability_tags: vec!["custom.arbitrary.ping".into()],
        effect: None,
        effectful: false,
        idempotent: true,
        dry_run_available: true,
        params: vec![],
        output_semantics: OutputSpec {
            fields: indexmap::indexmap! {
                "pong".into() => OutputField {
                    path: "$.pong".into(),
                    type_name: "bool".into(),
                    sensitivity: aelio_agent::types::Sensitivity::None,
                    meaning: "Whether the ping succeeded".into(),
                },
            },
            role_hint: Some("data".into()),
        },
        continuations: vec![],
        errors: vec![],
    });
    world.registry.register_ability(
        AbilityContract::effect("custom.arbitrary.ping")
            .with_tool_deps(vec!["custom_ping".into()])
            .with_postconditions(vec![Predicate::Present {
                path: "evidence.pong".into(),
            }]),
    );
    world.set_tool_host(Box::new(
        aelio_agent::abilities::invoke::MockToolHost::default().on("custom_ping", |_| {
            Ok(aelio_agent::Value::Map(indexmap::indexmap! {
                "pong".into() => aelio_agent::Value::Bool(true),
            }))
        }),
    ));

    let tenant_id = world.tenant.tenant_id.clone();
    let result = execute_path(
        &AbilityPath::seq(["custom.arbitrary.ping"]),
        ExecutionFrame::default(),
        &mut PathExecutionContext {
            registry: &world.registry,
            policies: &world.tenant.policies,
            policy: PolicyCtx {
                tenant: Some(tenant_id),
                ..Default::default()
            },
            personality: None,
            host: world.tool_host.as_mut(),
            signatures: &mut world.signatures,
            once_seen: &mut world.once_seen,
            effects: &mut world.effect_env,
            user_id: "u1",
            channel: "test",
        },
    )
    .expect("generic capability should execute");

    match result {
        PathExecution::Done {
            frame,
            invoked_tools,
        } => {
            assert_eq!(invoked_tools, vec!["custom_ping"]);
            assert_eq!(
                frame.evidence.get("pong"),
                Some(&aelio_agent::Value::Bool(true))
            );
        }
        PathExecution::NeedUser { .. } => panic!("parameterless tool must not suspend"),
    }
}

#[test]
fn composed_tools_pass_only_declared_sanitized_evidence_to_the_next_step() {
    use aelio_agent::blocks::executor::{
        execute_path, ExecutionFrame, PathExecution, PathExecutionContext,
    };
    use aelio_agent::contract::AbilityPath;
    use aelio_agent::policy::PolicyCtx;
    use aelio_agent::tenant::{OutputField, OutputSpec, ParamSource, ParamSpec, ToolSpec};
    use aelio_agent::types::Sensitivity;

    let mut world = World::demo_tenant("t_demo");
    world.registry.register_tool(ToolSpec {
        id: "resolve_customer".into(),
        name: "resolve_customer".into(),
        version: "1".into(),
        capability_tags: vec!["customer.resolve".into()],
        effect: None,
        effectful: false,
        idempotent: true,
        dry_run_available: true,
        params: vec![],
        output_semantics: OutputSpec {
            fields: indexmap::indexmap! {
                "customer_id".into() => OutputField {
                    path: "customer_id".into(),
                    type_name: "str".into(),
                    sensitivity: Sensitivity::None,
                    meaning: "resolved tenant customer identifier".into(),
                },
            },
            role_hint: Some("identifier".into()),
        },
        continuations: vec![],
        errors: vec![],
    });
    world.registry.register_tool(ToolSpec {
        id: "list_invoices".into(),
        name: "list_invoices".into(),
        version: "1".into(),
        capability_tags: vec!["invoice.list".into()],
        effect: None,
        effectful: false,
        idempotent: true,
        dry_run_available: true,
        params: vec![ParamSpec {
            name: "customer_id".into(),
            type_name: "str".into(),
            required: true,
            constraint: None,
            source: ParamSource::ToolOutput {
                ref_path: "customer_id".into(),
            },
            repair: None,
            prompt_hint: None,
            sensitivity: Sensitivity::None,
            default: None,
            depends_on: vec![],
        }],
        output_semantics: OutputSpec {
            fields: indexmap::indexmap! {
                "invoice_list".into() => OutputField {
                    path: "invoice_list".into(),
                    type_name: "list".into(),
                    sensitivity: Sensitivity::None,
                    meaning: "bounded invoices for the resolved customer".into(),
                },
            },
            role_hint: Some("data".into()),
        },
        continuations: vec![],
        errors: vec![],
    });
    world.set_tool_host(Box::new(
        aelio_agent::abilities::invoke::MockToolHost::default()
            .on("resolve_customer", |_| {
                Ok(aelio_agent::Value::Map(indexmap::indexmap! {
                    "customer_id".into() => aelio_agent::Value::str("customer-7"),
                }))
            })
            .on("list_invoices", |args| {
                if args.get("customer_id").and_then(aelio_agent::Value::as_str)
                    != Some("customer-7")
                {
                    return Err(aelio_agent::AelioError::new(
                        aelio_agent::ReasonCode::Missing,
                        "customer_id was not handed off",
                    ));
                }
                Ok(aelio_agent::Value::Map(indexmap::indexmap! {
                    "invoice_list".into() => aelio_agent::Value::List(vec![
                        aelio_agent::Value::str("inv-1"),
                        aelio_agent::Value::str("inv-2"),
                    ]),
                }))
            }),
    ));

    let result = execute_path(
        &AbilityPath::seq(["customer.resolve", "invoice.list"]),
        ExecutionFrame::default(),
        &mut PathExecutionContext {
            registry: &world.registry,
            policies: &world.tenant.policies,
            policy: PolicyCtx {
                tenant: Some(world.tenant.tenant_id.clone()),
                ..Default::default()
            },
            personality: None,
            host: world.tool_host.as_mut(),
            signatures: &mut world.signatures,
            once_seen: &mut world.once_seen,
            effects: &mut world.effect_env,
            user_id: "u1",
            channel: "test",
        },
    )
    .expect("the composed evidence handoff must execute");
    match result {
        PathExecution::Done {
            frame,
            invoked_tools,
        } => {
            assert_eq!(invoked_tools, ["resolve_customer", "list_invoices"]);
            assert!(matches!(
                frame.evidence.get("invoice_list"),
                Some(aelio_agent::Value::List(values)) if values.len() == 2
            ));
        }
        PathExecution::NeedUser { .. } => panic!("declared ToolOutput should satisfy the binder"),
    }
}

#[derive(Debug)]
struct CrossLanguageQueryEmbedder;

impl aelio_agent::embedding::Embedder for CrossLanguageQueryEmbedder {
    fn dimension(&self) -> usize {
        2
    }

    fn embed(&self, text: &str) -> aelio_agent::AelioResult<Vec<f32>> {
        let normalized = text.to_ascii_lowercase();
        if normalized.contains("avaricieux")
            || normalized.contains("crm clients query")
            || normalized.contains("greedy")
            || normalized.contains("haggler")
            || normalized.contains("price-sensitive")
            || normalized.contains("always negotiating")
            || normalized.contains("avaricious")
        {
            Ok(vec![1.0, 0.0])
        } else {
            Ok(vec![0.0, 1.0])
        }
    }

    fn supports_semantic_equivalence(&self) -> bool {
        true
    }
}

#[test]
fn non_english_rank_request_uses_closed_model_extraction_then_declared_execution() {
    let mut world = World::demo_tenant("t_demo");
    world
        .user_state
        .insert("u-fr".into(), "authenticated".into());
    world.set_embedder(Box::new(CrossLanguageQueryEmbedder));
    world.set_llm_provider(Box::new(
        aelio_agent::provider::ScriptedLlmProvider::deterministic().with_fallback(
            "aelio.understand.rank",
            r#"{"term":"avaricieux","limit":"10","operator":"most"}"#,
        ),
    ));

    let result = world.run_turn("u-fr", "donne-moi les clients les plus avaricieux");
    assert_eq!(result.tier, Some(LookupTier::Tier2));
    assert!(result
        .steps
        .iter()
        .any(|step| step.name == "Understand.Rank"));
    assert!(result
        .steps
        .iter()
        .any(|step| step.name == "Invoke.Call" && step.detail.contains("clients_query")));
    assert!(!result.reply.claim_refs.is_empty());
    assert_eq!(
        result.llm_calls, 2,
        "closed extraction plus grounded synthesis"
    );
}

/// A tier-0 deep procedure must execute its arbitrary capability, not fall through to greeting
/// rendering. This guards the production turn spine against becoming a worked-example router.
#[test]
fn tier_zero_deep_path_uses_generic_executor() {
    use aelio_agent::abilities::learn::{situation_hash, situation_key};
    use aelio_agent::abilities::registry::{
        ProcedureEvidence, ProcedureProvenance, ProcedureSpec, ProcedureStatus, SituationFilter,
    };
    use aelio_agent::contract::{AbilityContract, AbilityPath, Predicate};
    use aelio_agent::tenant::{OutputField, OutputSpec, ToolSpec};

    let mut world = World::demo_tenant("t_demo");
    world.tenant.states[0]
        .permission_envelope
        .push("telemetry.snapshot.read".into());
    world.registry.register_tool(ToolSpec {
        id: "read_snapshot".into(),
        name: "read_snapshot".into(),
        version: "1".into(),
        capability_tags: vec!["telemetry.snapshot.read".into()],
        effect: None,
        effectful: false,
        idempotent: true,
        dry_run_available: true,
        params: vec![],
        output_semantics: OutputSpec {
            fields: indexmap::indexmap! {
                "snapshot".into() => OutputField {
                    path: "$.snapshot".into(),
                    type_name: "str".into(),
                    sensitivity: aelio_agent::types::Sensitivity::None,
                    meaning: "The current telemetry snapshot".into(),
                },
            },
            role_hint: Some("data".into()),
        },
        continuations: vec![],
        errors: vec![],
    });
    world.registry.register_ability(
        AbilityContract::pure("telemetry.snapshot.read")
            .with_tool_deps(vec!["read_snapshot".into()])
            .with_postconditions(vec![Predicate::Present {
                path: "evidence.snapshot".into(),
            }]),
    );
    world.set_tool_host(Box::new(
        aelio_agent::abilities::invoke::MockToolHost::default().on("read_snapshot", |_| {
            Ok(aelio_agent::Value::Map(indexmap::indexmap! {
                "snapshot".into() => aelio_agent::Value::str("healthy"),
            }))
        }),
    ));

    let caps = world.tenant.states[0].permission_envelope.clone();
    let sigma = situation_key(
        "unauthenticated",
        "telemetry.snapshot.read",
        vec![],
        caps,
        None,
        0,
        None,
    );
    world.registry.register_procedure(ProcedureSpec {
        id: "snapshot-procedure".into(),
        version: "1".into(),
        tenant_id: "t_demo".into(),
        situation_hash: situation_hash(&sigma),
        situation_filter: SituationFilter {
            state: Some("unauthenticated".into()),
            intent_class: Some("telemetry.snapshot.read".into()),
            ..Default::default()
        },
        situation_embedding: vec![],
        path: AbilityPath::seq(["telemetry.snapshot.read"]),
        contract: AbilityContract::pure("snapshot-procedure")
            .with_tool_deps(vec!["read_snapshot".into()]),
        tool_deps: vec!["read_snapshot".into()],
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
            approved_by: None,
        },
        supersedes: None,
    });

    let result = world.run_turn("u1", "send a detailed telemetry snapshot now");
    assert_eq!(result.tier, Some(LookupTier::Tier0));
    assert_eq!(
        world.tool_host.invocation_count_for("read_snapshot"),
        Some(1)
    );
    assert!(result
        .steps
        .iter()
        .any(|step| step.name == "Invoke.Call" && step.detail.contains("read_snapshot")));
}

/// Trace: avaricious clients — polarity + confirm on thin margin.
#[test]
fn avaricious_clients_confirms_on_thin_margin() {
    let mut world = World::demo_tenant("t_demo");
    world.user_state.insert("u1".into(), "authenticated".into());
    let r = world.run_turn("u1", "give me the ten most avaricious clients");
    assert_eq!(r.depth, Depth::Deep);
    // Should confirm or resolve with discount_pressure
    let detail = r
        .steps
        .iter()
        .find(|s| s.name == "TermResolve")
        .map(|s| s.detail.clone())
        .unwrap_or_default();
    assert!(
        detail.contains("discount_pressure")
            || r.reply.text.to_lowercase().contains("discount")
            || r.reply.text.to_lowercase().contains("price")
            || r.reply.text.contains("Did you mean"),
        "detail={detail} reply={}",
        r.reply.text
    );
}

#[test]
fn persisted_term_confirmation_executes_the_exact_declared_plan() {
    let mut world = World::demo_tenant("t_demo");
    world.user_state.insert("u1".into(), "authenticated".into());
    world.user_flows.insert(
        "u1".into(),
        aelio_agent::blocks::flow::FlowInstance {
            flow_id: "__aelio.term_confirmation".into(),
            flow_version: "1".into(),
            current_step_idx: 0,
            slots: indexmap::indexmap! {
                "term".into() => serde_json::json!("avaricious"),
                "attribute".into() => serde_json::json!("discount_pressure_score"),
                "limit".into() => serde_json::json!(10),
                "want_most".into() => serde_json::json!(true),
            },
            attempts: 0,
            pinned_tool_versions: indexmap::IndexMap::new(),
            pinned_prompt_hashes: indexmap::IndexMap::new(),
            ttl_secs: Some(300),
            pending_step: Some("confirm".into()),
        },
    );

    let result = world.run_turn("u1", "yes");
    assert!(!result.suspended);
    assert!(result.active_flow.is_none());
    assert!(result.steps.iter().any(|step| {
        step.name == "TermConfirm"
            && step
                .detail
                .contains("avaricious -> discount_pressure_score")
    }));
    assert_eq!(
        world.tool_host.invocation_count_for("clients_query"),
        Some(1)
    );
}

/// SplitClauses pre-gate must not swallow multi-clause messages.
#[test]
fn multi_clause_not_swallowed_as_hi() {
    let (skipped, clauses) = aelio_agent::abilities::understand::split_clauses(
        "Hi, I need to cancel my order and also refund",
    );
    // Pure splitter may still skip LLM but must produce multiple clauses or deep path
    assert!(!clauses.is_empty());
    let depth = aelio_agent::abilities::understand::classify_depth(
        "Hi, I need to cancel my order and also refund",
    );
    assert_eq!(depth.depth, Depth::Deep);
    let _ = skipped;
}

#[test]
fn multi_clause_mutations_are_persisted_until_confirmation() {
    let mut world = World::demo_tenant("t_demo");
    let planned = world.run_turn("u-plan", "cancel my order and refund my payment");
    assert!(planned.suspended);
    assert_eq!(
        planned
            .active_flow
            .as_ref()
            .map(|flow| flow.flow_id.as_str()),
        Some("__aelio.multi_clause_confirmation")
    );
    assert_eq!(world.tool_host.invocation_count(), Some(0));

    let cancelled = world.run_turn("u-plan", "no");
    assert!(!cancelled.suspended);
    assert!(cancelled
        .reply
        .text
        .to_ascii_lowercase()
        .contains("cancelled"));
    assert_eq!(world.tool_host.invocation_count(), Some(0));
}

#[test]
fn confirmed_plan_keeps_remaining_actions_across_flow_suspension() {
    let mut world = World::demo_tenant("t_demo");
    let planned = world.run_turn("u-plan-resume", "log in and cancel my order");
    assert!(planned.suspended);

    let confirmed = world.run_turn("u-plan-resume", "yes");
    assert_eq!(
        confirmed
            .active_flow
            .as_ref()
            .map(|flow| flow.flow_id.as_str()),
        Some("login")
    );
    assert!(confirmed
        .active_flow
        .as_ref()
        .is_some_and(|flow| flow.slots.contains_key("__aelio_plan_next_index")));

    let phone = world.run_turn("u-plan-resume", "9876543210");
    assert!(phone.suspended);
    assert!(phone
        .active_flow
        .as_ref()
        .is_some_and(|flow| flow.slots.contains_key("__aelio_plan_clauses")));

    let completed = world.run_turn("u-plan-resume", "434543");
    assert_eq!(completed.new_state.as_deref(), Some("authenticated"));
    assert!(!world.user_flows.contains_key("u-plan-resume"));
}

/// Bind only asks for residual user params.
#[test]
fn binder_only_residual_phone() {
    use aelio_agent::abilities::bind::{resolve_all, BindSources};
    use aelio_agent::types::Value;

    let world = World::demo_tenant("t_demo");
    let tool = world.registry.lookup_tool("send_otp").unwrap();
    let sources = BindSources {
        env: indexmap::indexmap! { "now".into() => Value::str("2026-07-21T09:14:00Z") },
        ..Default::default()
    };
    let r = resolve_all(tool, &sources).unwrap();
    assert!(r.bound.contains_key("tenant_id"));
    assert_eq!(r.residual, vec!["phone".to_string()]);
}

/// Policy deny for unauthenticated destructive action.
#[test]
fn policy_denies_when_configured() {
    use aelio_agent::contract::Predicate;
    use aelio_agent::policy::{evaluate, PolicyCtx, PolicyDecision};
    use aelio_agent::tenant::{PolicyAction, PolicyEffect, PolicySpec, PolicySubject};

    let policies = vec![PolicySpec {
        id: "deny-cancel".into(),
        effect: PolicyEffect::Deny,
        subject: PolicySubject {
            state: Some("unauthenticated".into()),
            ..Default::default()
        },
        action: PolicyAction {
            capability: Some("orders.cancel".into()),
            ..Default::default()
        },
        condition: Predicate::True,
        reason_code: "auth_required".into(),
        priority: 100,
    }];
    let ctx = PolicyCtx {
        state: Some("unauthenticated".into()),
        capability: Some("orders.cancel".into()),
        ..Default::default()
    };
    assert!(matches!(
        evaluate(&policies, &ctx),
        PolicyDecision::Deny { .. }
    ));
}

/// Tool invalidation cascades to procedures.
#[test]
fn tool_invalidation_demotes_procedures() {
    use aelio_agent::abilities::learn::{propose_path_greeting, situation_hash, situation_key};
    use aelio_agent::abilities::registry::{
        ProcedureEvidence, ProcedureProvenance, ProcedureSpec, ProcedureStatus, SituationFilter,
    };
    use aelio_agent::contract::AbilityContract;

    let mut world = World::demo_tenant("t_demo");
    let sigma = situation_key("unauthenticated", "greeting", vec![], vec![], None, 0, None);
    let h = situation_hash(&sigma);
    world.registry.register_procedure(ProcedureSpec {
        id: "p_greet".into(),
        version: "1".into(),
        tenant_id: "t_demo".into(),
        situation_hash: h,
        situation_filter: SituationFilter {
            state: Some("unauthenticated".into()),
            intent_class: Some("greeting".into()),
            ..Default::default()
        },
        situation_embedding: vec![],
        path: propose_path_greeting(),
        contract: AbilityContract::pure("p_greet").with_tool_deps(vec!["send_otp".into()]),
        tool_deps: vec!["send_otp".into()],
        prompt_deps: vec![],
        evidence: ProcedureEvidence {
            observations: 10,
            success_rate: 1.0,
            mean_cost: 0.0,
            mean_latency_ms: 2.0,
        },
        status: ProcedureStatus::Promoted,
        provenance: ProcedureProvenance {
            origin: "test".into(),
            proposed_by: "test".into(),
            approved_by: None,
        },
        supersedes: None,
    });

    let demoted = world.registry.invalidate_tool("send_otp");
    assert!(demoted.contains(&"p_greet".to_string()));
    assert_eq!(
        world.registry.procedures.get("p_greet").unwrap().status,
        ProcedureStatus::Suspended
    );
}

/// Signature mismatch never applies wrong plan.
#[test]
fn sig_mismatch_does_not_guess() {
    use aelio_agent::abilities::sig::{self, ExtractionPlan, PlanStatus, SignatureRegistry};
    use aelio_agent::types::Value;

    let raw = Value::Map(indexmap::indexmap! {
        "otp".into() => Value::str("123456"),
    });
    let h = sig::hash(&sig::compute(&raw));
    let mut reg = SignatureRegistry::default();
    reg.plans.insert(
        "stale".into(),
        ExtractionPlan {
            sig_hash: "stale".into(),
            fields: indexmap::indexmap! {
                "code".into() => sig::ExtractField {
                    path: "code".into(),
                    type_name: "str".into(),
                },
            },
            status: PlanStatus::Promoted,
        },
    );
    assert!(sig::match_plan(&reg, &h).is_none());
}

/// learnable:false flows cannot be reordered.
#[test]
fn login_flow_not_learnable() {
    let world = World::demo_tenant("t_demo");
    let flow = world.registry.lookup_flow("login").unwrap();
    assert!(!flow.learnable);
    assert!(!aelio_agent::blocks::flow::may_reorder(flow));
}

/// Tier-2 composition: customer_id → invoices.
#[test]
fn tier2_composes_procedures() {
    use aelio_agent::abilities::learn::{compose, typecheck};
    use aelio_agent::abilities::registry::{
        ProcedureEvidence, ProcedureProvenance, ProcedureSpec, ProcedureStatus, SituationFilter,
    };
    use aelio_agent::contract::{AbilityContract, AbilityPath, Predicate};

    let mut world = World::demo_tenant("t_demo");
    world
        .registry
        .register_ability(AbilityContract::pure("Understand.Extract"));
    world
        .registry
        .register_ability(AbilityContract::pure("Recall.Lexical"));
    world
        .registry
        .register_ability(AbilityContract::pure("Invoke.Call"));
    world
        .registry
        .register_ability(AbilityContract::pure("Sum"));

    world.registry.register_procedure(ProcedureSpec {
        id: "P1".into(),
        version: "1".into(),
        tenant_id: "t".into(),
        situation_hash: "x".into(),
        situation_filter: SituationFilter {
            required_slots: vec!["person_or_org_name".into()],
            ..Default::default()
        },
        situation_embedding: vec![],
        path: AbilityPath::seq(["Understand.Extract", "Recall.Lexical"]),
        contract: AbilityContract::pure("P1").with_postconditions(vec![Predicate::Present {
            path: "customer_id".into(),
        }]),
        tool_deps: vec![],
        prompt_deps: vec![],
        evidence: ProcedureEvidence {
            observations: 5,
            success_rate: 1.0,
            ..Default::default()
        },
        status: ProcedureStatus::Promoted,
        provenance: ProcedureProvenance {
            origin: "tier2".into(),
            proposed_by: "test".into(),
            approved_by: None,
        },
        supersedes: None,
    });
    world.registry.register_procedure(ProcedureSpec {
        id: "P2".into(),
        version: "1".into(),
        tenant_id: "t".into(),
        situation_hash: "y".into(),
        situation_filter: SituationFilter {
            required_slots: vec!["customer_id".into()],
            ..Default::default()
        },
        situation_embedding: vec![],
        path: AbilityPath::seq(["Invoke.Call"]),
        contract: AbilityContract::pure("P2").with_postconditions(vec![Predicate::Present {
            path: "invoice_list".into(),
        }]),
        tool_deps: vec![],
        prompt_deps: vec![],
        evidence: ProcedureEvidence {
            observations: 5,
            success_rate: 1.0,
            ..Default::default()
        },
        status: ProcedureStatus::Promoted,
        provenance: ProcedureProvenance {
            origin: "tier2".into(),
            proposed_by: "test".into(),
            approved_by: None,
        },
        supersedes: None,
    });

    let path = compose(
        &world.registry,
        &aelio_agent::abilities::learn::GoalSpec {
            intent: "invoice_list".into(),
            produces: vec!["invoice_list".into()],
            available_slots: vec!["person_or_org_name".into()],
            allowed_capabilities: vec![],
            allow_effects: false,
        },
        aelio_agent::abilities::learn::CompositionBudget::default(),
    )
    .expect("should compose P1 → P2");
    assert!(typecheck(&world.registry, &path).is_ok());
    assert!(path.steps.len() >= 2);
}
