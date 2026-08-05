use std::collections::BTreeMap;
use std::sync::{Arc, Barrier};

use aelio_agent::adaptive::{
    AdaptiveArtifactHost, AdaptiveArtifactTurnV1, AdaptiveDecisionEnvelopeV1, ArtifactPinV1,
};
use aelio_agent::contract::AbilityPath;
use aelio_agent::runtime::{
    BehavioralOutcomeSignal, BehavioralSignalKind, ColdProposal, ColdProposalState,
    DependencySnapshot, DurableRuntime, DurableScheduler, DurableTurnRequest, ExplorationBudget,
    ExplorationPolicy, FlowCandidateState, IntentState, LearningColdLoop, PromotionGate,
    PromotionResult, ProposalEvidence, SagaReconciler, ScheduledJob, SideEffectIntent,
};
use aelio_agent::storage::{AelioStore, LogicalTable, RecordEnvelope, StoredRecord};
use aelio_agent::tenant::{
    FlowActivation, FlowEscape, FlowSpec, FlowStep, Preemption, ViolationAction,
};
use aelio_agent::types::{LookupTier, TenantMode};
use aelio_agent::{AelioError, Predicate, ReasonCode};
use aelio_db_query::Database;

fn store(tag: &str) -> AelioStore {
    let path = std::env::temp_dir().join(format!(
        "aelio_durable_workers_{tag}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let mut store = AelioStore::new(Database::create(path).unwrap(), 3).unwrap();
    store.migrate_tenant("tenant-1", 0).unwrap();
    store
}

struct VerifyingArtifactHost;

impl AdaptiveArtifactHost for VerifyingArtifactHost {
    fn verify_executable_pin(
        &mut self,
        tenant: &str,
        artifact: &ArtifactPinV1,
    ) -> Result<(), AelioError> {
        assert_eq!(tenant, "tenant-1");
        if artifact.id != "learned.followup.runtime" {
            return Err(AelioError::new(
                ReasonCode::NotFound,
                "test artifact is not admitted",
            ));
        }
        Ok(())
    }

    fn invoke(
        &mut self,
        _tenant: &str,
        _instance_id: &str,
        _decision: &AdaptiveDecisionEnvelopeV1,
    ) -> Result<AdaptiveArtifactTurnV1, AelioError> {
        Err(AelioError::new(
            ReasonCode::Unavailable,
            "activation verification must not execute the artifact",
        ))
    }
}

fn job(id: &str) -> ScheduledJob {
    ScheduledJob {
        id: id.into(),
        kind: "park_resume".into(),
        payload: serde_json::json!({"turn_id": "turn-1"}),
        state: aelio_agent::runtime::JobState::Scheduled,
        scheduled_at_ms: 10,
        owner: None,
        lease_expires_at_ms: None,
        attempts: 0,
        max_attempts: 3,
        last_error: None,
        terminal: None,
    }
}

fn proposal(id: &str, effectful: bool, evidence: ProposalEvidence) -> ColdProposal {
    ColdProposal {
        id: id.into(),
        situation_hash: format!("sigma-{id}"),
        situation_filter: Default::default(),
        path: AbilityPath::seq(["Express.Template"]),
        contract: aelio_agent::contract::AbilityContract::pure(format!("contract-{id}")),
        effectful,
        dependencies: DependencySnapshot {
            tool_versions: BTreeMap::from([("tool-a".into(), "1".into())]),
            prompt_hash: "none".into(),
        },
        evidence,
        state: ColdProposalState::Accumulating,
        promoted_version: None,
    }
}

fn passing_evidence() -> ProposalEvidence {
    ProposalEvidence {
        observations: 5,
        successes: 5,
        score_sum: 5.0,
        ..Default::default()
    }
}

fn learned_flow(id: &str, trigger: &str) -> FlowSpec {
    FlowSpec {
        id: id.into(),
        version: "1".into(),
        name: "Reviewed learned follow-up".into(),
        activation: FlowActivation {
            hard_preconditions: Vec::new(),
            trigger_surface: vec![trigger.into()],
            margin_threshold: 0.8,
        },
        learnable: true,
        preemption: Preemption::Hold,
        steps: vec![FlowStep {
            id: "respond".into(),
            intent: "respond".into(),
            postcondition: Predicate::True,
            admissible: vec!["crm.clients.query".into()],
            on_violation: ViolationAction::Escape,
            suspendable: false,
        }],
        escape: FlowEscape::FreeRange,
        terminal_states: vec!["authenticated".into()],
        ttl_secs: Some(300),
        max_attempts: 3,
        lowering: None,
    }
}

fn promote_source(
    shared: AelioStore,
    id: &str,
    situation_hash: &str,
    path: AbilityPath,
    evidence: ProposalEvidence,
    tool_dependency: Option<(&str, &str)>,
) -> String {
    let mut source = proposal(id, false, evidence);
    source.situation_hash = situation_hash.into();
    source.path = path;
    source.dependencies.tool_versions.clear();
    if let Some((tool_id, version)) = tool_dependency {
        source
            .dependencies
            .tool_versions
            .insert(tool_id.into(), version.into());
    }
    let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, shared);
    cold.submit_proposal(source, 0).unwrap();
    let version = match cold.promote(id, &PromotionGate::default(), 10).unwrap() {
        PromotionResult::Promoted { version } | PromotionResult::AlreadyPromoted { version } => {
            version
        }
    };
    format!("{id}@{version}")
}

#[test]
fn lease_contention_and_expiry_are_cas_safe() {
    let shared = store("leases");
    let mut first = DurableScheduler::new("tenant-1", shared.clone());
    let mut second = DurableScheduler::new("tenant-1", shared);
    first.schedule(job("job-1"), 0).unwrap();

    let lease = first.lease_due("worker-a", 10, 10, 1).unwrap();
    assert_eq!(lease.len(), 1);
    assert!(second.lease_due("worker-b", 19, 10, 1).unwrap().is_empty());

    let recovered = second.lease_due("worker-b", 20, 10, 1).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].job.owner.as_deref(), Some("worker-b"));
    assert_eq!(recovered[0].job.attempts, 2);
    assert_eq!(
        first.complete(&lease[0], "worker-a", 21).unwrap_err().code,
        ReasonCode::Conflict
    );
}

#[test]
fn worker_kind_filter_never_steals_another_workers_job() {
    let shared = store("kind_filter");
    let mut scheduler = DurableScheduler::new("tenant-1", shared);
    let mut proactive = job("proactive-1");
    proactive.kind = "proactive_delivery".into();
    scheduler.schedule(proactive, 0).unwrap();
    scheduler.schedule(job("resume-1"), 0).unwrap();

    let edge = scheduler
        .lease_due_kind("edge", 10, 100, 8, Some("proactive_delivery"))
        .unwrap();
    assert_eq!(edge.len(), 1);
    assert_eq!(edge[0].job.id, "proactive-1");

    let runtime = scheduler
        .lease_due_kind("runtime", 10, 100, 8, Some("park_resume"))
        .unwrap();
    assert_eq!(runtime.len(), 1);
    assert_eq!(runtime[0].job.id, "resume-1");
}

#[test]
fn crash_reconciliation_never_repeats_unknown_non_idempotent_effect() {
    let mut shared = store("reconcile");
    shared
        .put_if_absent(
            "tenant-1",
            LogicalTable::Turns,
            &RecordEnvelope {
                key: "turn-1".into(),
                kind: "turn".into(),
                status: "processing".into(),
                owner: "crashed-worker".into(),
                created_at_ms: 0,
                updated_at_ms: 0,
                expires_at_ms: None,
                value: serde_json::json!({
                    "user_id": "user-1",
                    "utterance": "charge account",
                    "result": null
                }),
            },
            None,
        )
        .unwrap();
    let mut reconciler = SagaReconciler::new("tenant-1", shared.clone());
    reconciler
        .record_intent(
            SideEffectIntent {
                id: "intent-1".into(),
                turn_id: "turn-1".into(),
                tool_id: "charge".into(),
                tool_version: "1".into(),
                idempotency_key: None,
                idempotent: false,
                effectful: true,
                dry_run: false,
                state: IntentState::Pending,
                attempts: 1,
                max_attempts: 3,
                terminal_reason: None,
            },
            0,
        )
        .unwrap();
    // Provider receipts intentionally share this logical table with tool-call receipts.
    // Reconciliation must filter by envelope kind before typed deserialization.
    shared
        .put_if_absent(
            "tenant-1",
            LogicalTable::CallRecords,
            &RecordEnvelope {
                key: "turn-1:call:000".into(),
                kind: "provider_call".into(),
                status: "recorded".into(),
                owner: "runtime".into(),
                created_at_ms: 0,
                updated_at_ms: 0,
                expires_at_ms: None,
                value: serde_json::json!({"turn_id": "turn-1", "call": {"kind": "mock"}}),
            },
            None,
        )
        .unwrap();

    let outcomes = reconciler.reconcile(10, 10).unwrap();
    assert!(outcomes.iter().any(|outcome| matches!(
        outcome,
        aelio_agent::runtime::ReconcileOutcome::NonIdempotentManualReview { .. }
    )));
    assert!(outcomes.iter().any(|outcome| matches!(
        outcome,
        aelio_agent::runtime::ReconcileOutcome::TurnManualReview { .. }
    )));
    let intent: StoredRecord<SideEffectIntent> = shared
        .get("tenant-1", LogicalTable::EffectIntents, "intent-1")
        .unwrap()
        .unwrap();
    assert_eq!(intent.envelope.value.state, IntentState::ManualReview);
}

#[test]
fn promotion_race_creates_one_immutable_version() {
    let shared = store("promotion_race");
    let mut setup = LearningColdLoop::new("tenant-1", TenantMode::Bake, shared.clone());
    setup
        .submit_proposal(proposal("proposal-1", false, passing_evidence()), 0)
        .unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let store = shared.clone();
            std::thread::spawn(move || {
                let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, store);
                barrier.wait();
                cold.promote("proposal-1", &PromotionGate::default(), 10)
                    .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert!(results
        .iter()
        .any(|result| matches!(result, PromotionResult::Promoted { .. })));
    let procedures: Vec<StoredRecord<aelio_agent::runtime::PromotedProcedureVersion>> = shared
        .list("tenant-1", LogicalTable::Procedures, Some("promoted"), 10)
        .unwrap();
    assert_eq!(procedures.len(), 1);
    let proposal: StoredRecord<ColdProposal> = shared
        .get("tenant-1", LogicalTable::Proposals, "proposal-1")
        .unwrap()
        .unwrap();
    assert_eq!(proposal.envelope.status, "promoted");
    assert_eq!(
        procedures[0].envelope.value.spec.version,
        proposal.envelope.value.promoted_version.clone().unwrap()
    );
}

#[test]
fn promoted_procedure_can_learn_a_ranked_immutable_v2() {
    let shared = store("procedure_v2");
    let mut initial = proposal("evolving", false, passing_evidence());
    initial.dependencies.tool_versions.clear();
    let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, shared.clone());
    cold.submit_proposal(initial, 0).unwrap();
    let first = match cold
        .promote("evolving", &PromotionGate::default(), 10)
        .unwrap()
    {
        PromotionResult::Promoted { version } => version,
        PromotionResult::AlreadyPromoted { .. } => unreachable!(),
    };
    for index in 0..5 {
        cold.record_signal(BehavioralOutcomeSignal {
            id: format!("v2-signal-{index}"),
            turn_id: format!("v2-turn-{index}"),
            proposal_id: Some("evolving".into()),
            kind: BehavioralSignalKind::TurnCompleted,
            step_ids: vec!["Express.Template".into()],
            failed_step: None,
            latency_ms: 1,
            token_cost: 0,
            call_cost_microunits: 0,
            observed_at_ms: 20 + index,
        })
        .unwrap();
    }
    let second = match cold
        .promote("evolving", &PromotionGate::default(), 30)
        .unwrap()
    {
        PromotionResult::Promoted { version } => version,
        PromotionResult::AlreadyPromoted { .. } => panic!("new evidence must produce v2"),
    };
    assert_ne!(first, second);
    let versions: Vec<StoredRecord<aelio_agent::runtime::PromotedProcedureVersion>> = shared
        .list("tenant-1", LogicalTable::Procedures, Some("promoted"), 10)
        .unwrap();
    assert_eq!(versions.len(), 2);
    let v2 = versions
        .iter()
        .find(|row| row.envelope.value.version == second)
        .unwrap();
    assert_eq!(
        v2.envelope.value.spec.supersedes.as_deref(),
        Some(first.as_str())
    );

    let runtime = DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared).unwrap();
    assert_eq!(
        runtime.world.registry.procedures["evolving"].version, second,
        "the strongest immutable version must win active retrieval"
    );
}

#[test]
fn promoted_procedure_is_tier_zero_after_database_restart() {
    let path = std::env::temp_dir().join(format!(
        "aelio_promoted_restart_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::create_dir_all(&path).unwrap();
    let mut shared = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
    shared.migrate_tenant("tenant-1", 0).unwrap();
    let world = aelio_agent::World::demo_tenant("tenant-1");
    let sigma = aelio_agent::abilities::learn::situation_key(
        "unauthenticated",
        "greeting",
        vec![],
        world.tenant.states[0].permission_envelope.clone(),
        None,
        0,
        None,
    );
    let mut durable_proposal = proposal("restart-procedure", false, passing_evidence());
    durable_proposal.dependencies.tool_versions.clear();
    durable_proposal.situation_hash = aelio_agent::abilities::learn::situation_hash(&sigma);
    durable_proposal.situation_filter.state = Some("unauthenticated".into());
    durable_proposal.situation_filter.intent_class = Some("greeting".into());
    let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, shared.clone());
    cold.submit_proposal(durable_proposal, 0).unwrap();
    let version = match cold
        .promote("restart-procedure", &PromotionGate::default(), 10)
        .unwrap()
    {
        PromotionResult::Promoted { version } => version,
        PromotionResult::AlreadyPromoted { .. } => unreachable!(),
    };
    shared.flush().unwrap();
    drop(cold);
    drop(shared);

    let reopened = AelioStore::new(Database::open(&path).unwrap(), 3).unwrap();
    let mut runtime = aelio_agent::runtime::DurableRuntime::new(
        aelio_agent::World::demo_tenant("tenant-1"),
        reopened,
    )
    .unwrap();
    let result = runtime
        .run_turn(aelio_agent::runtime::DurableTurnRequest {
            turn_id: "restart-tier-turn".into(),
            user_id: "restart-user".into(),
            utterance: "Hi".into(),
        })
        .unwrap();
    assert_eq!(result.tier, Some(LookupTier::Tier0));
    assert_eq!(result.llm_calls, 0);
    let loaded = runtime
        .world
        .registry
        .lookup_procedure_by_hash(&aelio_agent::abilities::learn::situation_hash(&sigma))
        .unwrap();
    assert_eq!(loaded.version, version);
}

#[test]
fn effectful_promotion_requires_explicit_approval() {
    let shared = store("approval");
    let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, shared);
    cold.submit_proposal(proposal("effectful", true, passing_evidence()), 0)
        .unwrap();
    assert_eq!(
        cold.promote("effectful", &PromotionGate::default(), 10)
            .unwrap_err()
            .code,
        ReasonCode::GateNotMet
    );
    cold.approve("effectful", "tenant-admin", 11).unwrap();
    assert!(matches!(
        cold.promote("effectful", &PromotionGate::default(), 12)
            .unwrap(),
        PromotionResult::Promoted { .. }
    ));
}

#[test]
fn approval_cannot_be_preseeded_for_a_nonexistent_future_proposal() {
    let shared = store("approval_preseed");
    let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, shared);
    assert_eq!(
        cold.approve("not-yet-proposed", "tenant-admin", 10)
            .unwrap_err()
            .code,
        ReasonCode::NotFound
    );
}

#[test]
fn dependency_changes_invalidate_without_mutating_promoted_version() {
    let shared = store("invalidation");
    let mut prompt_v1_registry = aelio_agent::abilities::registry::Registry::default();
    let mut prompt_v1 = aelio_agent::AbilityContract::pure("Express.Template");
    prompt_v1.prompt_hash = Some("prompt-v1".into());
    prompt_v1_registry.register_ability(prompt_v1);
    let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, shared.clone());
    cold.submit_proposal(proposal("dependent", false, passing_evidence()), 0)
        .unwrap();
    let version = match cold
        .promote("dependent", &PromotionGate::default(), 10)
        .unwrap()
    {
        PromotionResult::Promoted { version } => version,
        PromotionResult::AlreadyPromoted { .. } => unreachable!(),
    };
    let immutable_key = format!("dependent@{version}");
    let invalidated = cold
        .invalidate_dependencies(
            &BTreeMap::from([("tool-a".into(), "2".into())]),
            &prompt_v1_registry,
            20,
        )
        .unwrap();
    assert_eq!(invalidated.len(), 1);
    assert!(!cold.is_valid(&immutable_key).unwrap());
    let stored: StoredRecord<aelio_agent::runtime::PromotedProcedureVersion> = shared
        .get("tenant-1", LogicalTable::Procedures, &immutable_key)
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.envelope.value.dependencies.tool_versions["tool-a"],
        "1"
    );

    let mut prompt_dependent = proposal("prompt-dependent", false, passing_evidence());
    prompt_dependent.dependencies.prompt_hash =
        aelio_agent::abilities::learn::path_prompt_dependency_fingerprint(
            &prompt_v1_registry,
            &prompt_dependent.path,
        );
    cold.submit_proposal(prompt_dependent, 21).unwrap();
    let prompt_version = match cold
        .promote("prompt-dependent", &PromotionGate::default(), 22)
        .unwrap()
    {
        PromotionResult::Promoted { version } => version,
        PromotionResult::AlreadyPromoted { .. } => unreachable!(),
    };
    let prompt_key = format!("prompt-dependent@{prompt_version}");
    let mut prompt_v2_registry = aelio_agent::abilities::registry::Registry::default();
    let mut prompt_v2 = aelio_agent::AbilityContract::pure("Express.Template");
    prompt_v2.prompt_hash = Some("prompt-v2".into());
    prompt_v2_registry.register_ability(prompt_v2);
    let prompt_invalidated = cold
        .invalidate_dependencies(
            &BTreeMap::from([("tool-a".into(), "1".into())]),
            &prompt_v2_registry,
            23,
        )
        .unwrap();
    assert!(prompt_invalidated
        .iter()
        .any(|item| item.procedure_version == prompt_key
            && item.reason == "prompt dependency fingerprint changed"));
}

#[test]
fn repair_is_a_negative_signal_with_failed_step_credit() {
    let shared = store("repair");
    let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, shared.clone());
    cold.submit_proposal(
        proposal("repair-proposal", false, ProposalEvidence::default()),
        0,
    )
    .unwrap();
    cold.record_signal(BehavioralOutcomeSignal {
        id: "signal-repair".into(),
        turn_id: "turn-1".into(),
        proposal_id: Some("repair-proposal".into()),
        kind: BehavioralSignalKind::UserRepair,
        step_ids: vec!["select".into(), "invoke".into()],
        failed_step: Some("invoke".into()),
        latency_ms: 100,
        token_cost: 20,
        call_cost_microunits: 30,
        observed_at_ms: 10,
    })
    .unwrap();
    let stored: StoredRecord<ColdProposal> = shared
        .get("tenant-1", LogicalTable::Proposals, "repair-proposal")
        .unwrap()
        .unwrap();
    assert!(stored.envelope.value.evidence.score_sum < 0.0);
    assert_eq!(
        stored.envelope.value.evidence.step_credit_sum["invoke"],
        -1.0
    );
}

#[test]
fn exploration_is_bounded_and_never_live_effectful() {
    let cold = LearningColdLoop::new("tenant-1", TenantMode::Live, store("exploration"));
    let mut budget = ExplorationBudget {
        max_trials: 1,
        used_trials: 0,
    };
    cold.claim_exploration(&mut budget, false, false).unwrap();
    assert_eq!(
        cold.claim_exploration(&mut budget, false, false)
            .unwrap_err()
            .code,
        ReasonCode::BudgetExceeded
    );
}

/// Greetings no longer traverse the cold learning path: the Conductor answers them from a stored
/// harness on turn one. This is the replacement guarantee for the old "authored greeting learning
/// must not manufacture model-token cost" assertion — the cost is now zero because no model runs at
/// all, not because a learned procedure kept it at zero. See FLAGS F-024.
#[test]
fn conductor_answers_greetings_without_cold_path_or_model_cost() {
    let shared = store("conductor_greeting_contract");
    let mut runtime =
        DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared.clone()).unwrap();
    for index in 0..3 {
        let result = runtime
            .run_turn(DurableTurnRequest {
                turn_id: format!("greet-{index}"),
                user_id: format!("greet-user-{index}"),
                utterance: "hi".into(),
            })
            .unwrap();
        assert_eq!(
            result.llm_calls, 0,
            "greeting turn {index} must stay model-free"
        );
        assert!(
            !result.steps.iter().any(|s| s.name == "ProposePath"),
            "greeting turn {index} must not cold-propose"
        );
        assert!(
            result.steps.iter().any(|s| s.name.starts_with("Harness.")),
            "greeting turn {index} must be answered by a harness"
        );
    }
    // No procedure is learned for greetings any more: the deterministic harness already is the
    // fast path, so there is nothing for the promotion gate to warm up.
    let procedures: Vec<StoredRecord<aelio_agent::runtime::PromotedProcedureVersion>> = shared
        .list("tenant-1", LogicalTable::Procedures, Some("promoted"), 10)
        .unwrap();
    assert!(
        procedures.is_empty(),
        "greetings must not create learned procedures once Conductor owns them"
    );
}

#[test]
fn repeated_success_promotes_durably_and_next_turn_is_tier_zero() {
    let shared = store("automatic_promotion");
    let mut runtime =
        DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared.clone()).unwrap();
    // A cold-path utterance: the Conductor escalates it, so it still exercises the learning ladder.
    // Greetings are deliberately excluded now — see `conductor_answers_greetings_without_cold_path_or_model_cost`.
    for index in 0..5 {
        let result = runtime
            .run_turn(DurableTurnRequest {
                turn_id: format!("cold-{index}"),
                user_id: format!("user-{index}"),
                utterance: "list jobs".into(),
            })
            .unwrap();
        assert_eq!(result.tier, Some(LookupTier::Tier2));
    }
    let procedures: Vec<StoredRecord<aelio_agent::runtime::PromotedProcedureVersion>> = shared
        .list("tenant-1", LogicalTable::Procedures, Some("promoted"), 10)
        .unwrap();
    assert_eq!(procedures.len(), 1);
    let spec = &procedures[0].envelope.value.spec;
    assert_eq!(
        spec.situation_filter.state.as_deref(),
        Some("unauthenticated")
    );
    assert_eq!(
        spec.situation_filter.intent_class.as_deref(),
        Some("greeting")
    );
    assert!(!spec.situation_embedding.is_empty());
    assert!(!spec.contract.postconditions.is_empty());

    let warm = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "warm".into(),
            user_id: "warm-user".into(),
            utterance: "list jobs".into(),
        })
        .unwrap();
    assert_eq!(warm.tier, Some(LookupTier::Tier0));
    assert_eq!(warm.llm_calls, 0);
}

#[test]
fn unified_world_never_hydrates_or_persists_legacy_flow_instances() {
    let shared = store("unified_no_legacy_flow_state");
    let mut world = aelio_agent::World::demo_tenant("tenant-1");
    world.disable_legacy_flow_execution();
    let mut runtime = DurableRuntime::new(world, shared.clone()).unwrap();
    runtime
        .run_turn(DurableTurnRequest {
            turn_id: "unified-no-flow-turn".into(),
            user_id: "unified-user".into(),
            utterance: "hi".into(),
        })
        .unwrap();
    let legacy: Option<StoredRecord<Option<aelio_agent::blocks::flow::FlowInstance>>> = shared
        .get("tenant-1", LogicalTable::FlowInstances, "unified-user")
        .unwrap();
    assert!(legacy.is_none());
    assert!(runtime.world.user_flows.is_empty());
}

#[test]
fn normalized_rank_query_learns_through_the_same_tier_ladder() {
    let shared = store("rank_tier_ladder");
    let mut world = aelio_agent::World::demo_tenant("tenant-1");
    world.tenant.states[0]
        .permission_envelope
        .push("crm.clients.query".into());
    let mut runtime = DurableRuntime::new(world, shared).unwrap();
    for index in 0..5 {
        let result = runtime
            .run_turn(DurableTurnRequest {
                turn_id: format!("rank-cold-{index}"),
                user_id: format!("rank-user-{index}"),
                utterance: "give me the ten most avaricious clients".into(),
            })
            .unwrap();
        assert_eq!(result.tier, Some(LookupTier::Tier2));
        assert!(result.steps.iter().any(|step| step.name == "Invoke.Call"));
        assert!(!result.reply.claim_refs.is_empty());
    }
    let warm = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "rank-warm".into(),
            user_id: "rank-user-new".into(),
            utterance: "give me the ten most avaricious clients".into(),
        })
        .unwrap();
    assert_eq!(warm.tier, Some(LookupTier::Tier0));
    assert_eq!(
        warm.llm_calls, 1,
        "only grounded terminal synthesis remains"
    );
    assert!(warm.steps.iter().all(|step| step.name != "Tier2Compose"));
}

#[test]
fn distinct_term_confirmations_promote_a_durable_tenant_synonym() {
    let shared = store("term_synonym_learning");
    let mut runtime =
        DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared.clone()).unwrap();
    for index in 0..3 {
        let user_id = format!("term-user-{index}");
        runtime
            .world
            .user_state
            .insert(user_id.clone(), "authenticated".into());
        runtime.world.user_flows.insert(
            user_id.clone(),
            aelio_agent::blocks::flow::FlowInstance {
                flow_id: "__aelio.term_confirmation".into(),
                flow_version: "1".into(),
                current_step_idx: 0,
                slots: indexmap::indexmap! {
                    "term".into() => serde_json::json!("parsimonious"),
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
        let result = runtime
            .run_turn(DurableTurnRequest {
                turn_id: format!("term-confirm-{index}"),
                user_id,
                utterance: "yes".into(),
            })
            .unwrap();
        assert!(result.steps.iter().any(|step| step.name == "TermLearn"));
    }
    assert!(runtime.world.tenant.attributes[0]
        .learned
        .iter()
        .any(|term| term == "parsimonious"));

    drop(runtime);
    let restarted =
        DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared).unwrap();
    assert!(restarted.world.tenant.attributes[0]
        .learned
        .iter()
        .any(|term| term == "parsimonious"));
}

#[test]
fn live_shadow_exploration_is_read_only_bounded_and_restart_durable() {
    let shared = store("live_shadow");
    let world = aelio_agent::World::demo_tenant("tenant-1");
    let sigma = aelio_agent::abilities::learn::situation_key(
        "unauthenticated",
        "greeting",
        vec![],
        world.tenant.states[0].permission_envelope.clone(),
        None,
        0,
        None,
    );
    let situation_hash = aelio_agent::abilities::learn::situation_hash(&sigma);
    promote_source(
        shared.clone(),
        "primary-greeting",
        &situation_hash,
        AbilityPath::seq(["Express.Template"]),
        passing_evidence(),
        None,
    );
    promote_source(
        shared.clone(),
        "runner-up-greeting",
        &situation_hash,
        AbilityPath::seq(["Registry.Capabilities"]),
        ProposalEvidence {
            observations: 5,
            successes: 4,
            score_sum: 4.0,
            ..Default::default()
        },
        None,
    );

    let mut runtime = DurableRuntime::new(world, shared.clone()).unwrap();
    runtime
        .configure_exploration(ExplorationPolicy {
            enabled: true,
            sample_rate_bps: 10_000,
            max_trials_per_window: 1,
            window_ms: 3_600_000,
            max_candidate_steps: 8,
        })
        .unwrap();
    let first = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "shadow-turn-1".into(),
            user_id: "shadow-user-1".into(),
            utterance: "hi".into(),
        })
        .unwrap();
    assert_eq!(first.tier, Some(LookupTier::Tier0));
    assert_eq!(first.llm_calls, 0);
    assert!(first.steps.iter().any(|step| {
        step.name == "Explore.Shadow" && step.detail.contains("runner-up-greeting")
    }));
    assert_eq!(first.new_state, None);
    let comparisons: Vec<StoredRecord<aelio_agent::runtime::ExplorationComparison>> = shared
        .list("tenant-1", LogicalTable::Explorations, Some("recorded"), 10)
        .unwrap();
    assert_eq!(comparisons.len(), 1);
    assert!(comparisons[0]
        .envelope
        .value
        .candidate_procedure
        .starts_with("runner-up-greeting@"));
    assert!(comparisons[0].envelope.value.invoked_tools.is_empty());

    let second = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "shadow-turn-2".into(),
            user_id: "shadow-user-2".into(),
            utterance: "hi".into(),
        })
        .unwrap();
    assert_eq!(second.tier, Some(LookupTier::Tier0));
    assert!(!second
        .steps
        .iter()
        .any(|step| step.name == "Explore.Shadow"));

    drop(runtime);
    let mut restarted =
        DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared.clone()).unwrap();
    let third = restarted
        .run_turn(DurableTurnRequest {
            turn_id: "shadow-turn-3".into(),
            user_id: "shadow-user-3".into(),
            utterance: "hi".into(),
        })
        .unwrap();
    assert_eq!(third.tier, Some(LookupTier::Tier0));
    assert!(!third.steps.iter().any(|step| step.name == "Explore.Shadow"));
    let comparisons: Vec<StoredRecord<aelio_agent::runtime::ExplorationComparison>> = shared
        .list("tenant-1", LogicalTable::Explorations, Some("recorded"), 10)
        .unwrap();
    assert_eq!(comparisons.len(), 1);
}

#[test]
fn concurrent_live_exploration_claims_cannot_exceed_the_shared_cap() {
    let shared = store("exploration_cap_race");
    let policy = ExplorationPolicy {
        enabled: true,
        sample_rate_bps: 10_000,
        max_trials_per_window: 1,
        window_ms: 3_600_000,
        max_candidate_steps: 8,
    };
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|index| {
            let barrier = Arc::clone(&barrier);
            let store = shared.clone();
            let policy = policy.clone();
            std::thread::spawn(move || {
                let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Live, store);
                barrier.wait();
                cold.claim_live_exploration(&format!("race-{index}"), &policy, 1_000)
                    .unwrap()
            })
        })
        .collect();
    let claims = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .filter(|claimed| *claimed)
        .count();
    assert_eq!(claims, 1);
}

#[test]
fn candidate_flow_requires_review_preserves_protected_rails_and_survives_restart() {
    let shared = store("candidate_flow");
    let source_key = promote_source(
        shared.clone(),
        "flow-source",
        "flow-source-situation",
        AbilityPath::seq(["crm.clients.query"]),
        passing_evidence(),
        Some(("clients_query", "1")),
    );
    let mut runtime =
        DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared.clone()).unwrap();

    assert_eq!(
        runtime
            .propose_candidate_flow(
                learned_flow("login-lookalike", "log in"),
                vec![source_key.clone()]
            )
            .unwrap_err()
            .code,
        ReasonCode::PolicyDenied
    );
    runtime
        .propose_candidate_flow(
            learned_flow("learned-followup", "run learned followup"),
            vec![source_key],
        )
        .unwrap();
    let pending = runtime
        .list_candidate_flows(Some("pending_review"), 10)
        .unwrap();
    assert_eq!(pending.len(), 1);
    let key = pending[0].envelope.key.clone();
    assert!(!pending[0].envelope.value.flow.learnable);
    assert_eq!(
        runtime
            .activate_candidate_flow(&key, false)
            .unwrap_err()
            .code,
        ReasonCode::PolicyDenied
    );
    runtime
        .review_candidate_flow(
            &key,
            true,
            "tenant-admin",
            Some("reviewed against authored rails".into()),
        )
        .unwrap();
    runtime.activate_candidate_flow(&key, false).unwrap();
    assert!(runtime
        .world
        .tenant
        .flows
        .iter()
        .any(|flow| flow.id == "learned-followup" && !flow.learnable));
    let active = runtime.list_candidate_flows(Some("active"), 10).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].envelope.value.state, FlowCandidateState::Active);
    let entered = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "learned-flow-turn".into(),
            user_id: "learned-flow-user".into(),
            utterance: "run learned followup".into(),
        })
        .unwrap();
    assert!(entered.suspended);
    assert_eq!(
        entered
            .active_flow
            .as_ref()
            .map(|instance| instance.flow_id.as_str()),
        Some("learned-followup")
    );
    assert!(entered.steps.iter().any(|step| {
        step.name == "Registry.LookupTool" && step.detail.contains("crm.clients.query")
    }));

    drop(runtime);
    let restarted =
        DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared).unwrap();
    assert!(restarted
        .world
        .tenant
        .flows
        .iter()
        .any(|flow| flow.id == "learned-followup"));
}

#[test]
fn unified_candidate_flow_stays_approved_until_an_executable_artifact_is_bound() {
    let shared = store("candidate_flow_materialization_gate");
    let source_key = promote_source(
        shared.clone(),
        "flow-source-materialized",
        "flow-source-materialized-situation",
        AbilityPath::seq(["crm.clients.query"]),
        passing_evidence(),
        Some(("clients_query", "1")),
    );
    let mut world = aelio_agent::World::demo_tenant("tenant-1");
    world.disable_legacy_flow_execution();
    world.set_adaptive_artifact_host(Box::new(VerifyingArtifactHost));
    let mut runtime = DurableRuntime::new(world, shared).unwrap();
    runtime
        .propose_candidate_flow(
            learned_flow("learned-followup-materialized", "run materialized followup"),
            vec![source_key],
        )
        .unwrap();
    let key = runtime
        .list_candidate_flows(Some("pending_review"), 10)
        .unwrap()[0]
        .envelope
        .key
        .clone();
    runtime
        .review_candidate_flow(&key, true, "tenant-admin", None)
        .unwrap();

    let error = runtime.activate_candidate_flow(&key, false).unwrap_err();
    assert_eq!(error.code, ReasonCode::GateNotMet);
    let approved = runtime.list_candidate_flows(Some("approved"), 10).unwrap();
    assert_eq!(approved.len(), 1);
    assert_eq!(
        approved[0].envelope.value.state,
        FlowCandidateState::Approved
    );
    assert!(!runtime
        .world
        .tenant
        .flows
        .iter()
        .any(|flow| flow.id == "learned-followup-materialized"));

    let artifact = ArtifactPinV1 {
        id: "learned.followup.runtime".into(),
        version: 1,
        hash: "ab".repeat(32),
    };
    runtime
        .activate_candidate_flow_with_artifact(&key, false, Some(artifact.clone()))
        .unwrap();
    assert_eq!(
        runtime
            .world
            .tenant
            .flow_artifacts
            .get("learned-followup-materialized"),
        Some(&artifact)
    );
    assert_eq!(
        runtime.list_candidate_flows(Some("active"), 10).unwrap()[0]
            .envelope
            .value
            .state,
        FlowCandidateState::Active
    );
}

#[test]
fn candidate_flow_review_is_single_winner_under_contention() {
    let shared = store("candidate_review_race");
    let source_key = promote_source(
        shared.clone(),
        "race-flow-source",
        "race-flow-situation",
        AbilityPath::seq(["crm.clients.query"]),
        passing_evidence(),
        Some(("clients_query", "1")),
    );
    let mut runtime =
        DurableRuntime::new(aelio_agent::World::demo_tenant("tenant-1"), shared.clone()).unwrap();
    runtime
        .propose_candidate_flow(
            learned_flow("review-race", "run review race"),
            vec![source_key],
        )
        .unwrap();
    let key = runtime
        .list_candidate_flows(Some("pending_review"), 1)
        .unwrap()[0]
        .envelope
        .key
        .clone();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [true, false]
        .into_iter()
        .map(|approved| {
            let barrier = Arc::clone(&barrier);
            let store = shared.clone();
            let key = key.clone();
            std::thread::spawn(move || {
                let mut cold = LearningColdLoop::new("tenant-1", TenantMode::Bake, store);
                barrier.wait();
                cold.review_candidate_flow(&key, approved, "reviewer", None, 20)
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .filter(|error| error.code == ReasonCode::Conflict)
            .count(),
        1
    );
}

#[derive(Debug)]
struct ConstantSemanticEmbedder(&'static str);

impl aelio_agent::embedding::Embedder for ConstantSemanticEmbedder {
    fn dimension(&self) -> usize {
        3
    }

    fn embed(&self, _text: &str) -> aelio_agent::AelioResult<Vec<f32>> {
        Ok(vec![0.0, 1.0, 0.0])
    }

    fn supports_semantic_equivalence(&self) -> bool {
        true
    }

    fn space_id(&self) -> String {
        format!("test.constant:{}", self.0)
    }
}

#[test]
fn durable_promotion_and_tier_one_use_the_same_configured_embedding_space() {
    let shared = store("shared_embedding_space");
    let embedder: Arc<dyn aelio_agent::embedding::Embedder> =
        Arc::new(ConstantSemanticEmbedder("v1"));
    let mut runtime = DurableRuntime::new_with_embedder(
        aelio_agent::World::demo_tenant("tenant-1"),
        shared.clone(),
        embedder,
    )
    .unwrap();
    // Cold-path utterance so the promotion ladder still runs; greetings are Conductor-owned now.
    for index in 0..5 {
        runtime
            .run_turn(DurableTurnRequest {
                turn_id: format!("semantic-cold-{index}"),
                user_id: format!("semantic-user-{index}"),
                utterance: "list jobs".into(),
            })
            .unwrap();
    }
    let procedures: Vec<StoredRecord<aelio_agent::runtime::PromotedProcedureVersion>> = shared
        .list("tenant-1", LogicalTable::Procedures, Some("promoted"), 10)
        .unwrap();
    assert_eq!(procedures.len(), 1);
    assert_eq!(
        procedures[0].envelope.value.spec.situation_embedding,
        vec![0.0, 1.0, 0.0]
    );

    let near = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "semantic-near".into(),
            user_id: "semantic-user-0".into(),
            utterance: "list jobs".into(),
        })
        .unwrap();
    assert_eq!(near.tier, Some(LookupTier::Tier1));
    assert_eq!(near.llm_calls, 0);
}

#[test]
fn persisted_embedding_space_rejects_equal_dimension_model_drift() {
    let shared = store("embedding_space_drift");
    let runtime = DurableRuntime::new_with_embedder(
        aelio_agent::World::demo_tenant("tenant-1"),
        shared.clone(),
        Arc::new(ConstantSemanticEmbedder("v1")),
    )
    .unwrap();
    drop(runtime);
    let error = match DurableRuntime::new_with_embedder(
        aelio_agent::World::demo_tenant("tenant-1"),
        shared,
        Arc::new(ConstantSemanticEmbedder("v2")),
    ) {
        Ok(_) => panic!("equal dimensions must not hide an embedding-model change"),
        Err(error) => error,
    };
    assert_eq!(error.code, ReasonCode::Conflict);
}
