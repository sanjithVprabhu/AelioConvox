use aelio_agent::contract::Predicate;
use aelio_agent::documents::{
    DocumentMutationContext, DocumentNamespace, DocumentQuota, DocumentSection, VirtualDocument,
    VirtualDocumentStore,
};
use aelio_agent::embedding::HashEmbedder;
use aelio_agent::memory::{
    DecayPolicy, Memory, MemoryKind, MemoryProvenance, MemoryShape, MemoryStore, ValidTime,
};
use aelio_agent::policy::PolicyCtx;
use aelio_agent::runtime::{
    DurableRuntime, DurableTurnRequest, ProactiveCandidate, ProactiveDecision, ProactiveLoop,
    ProactivePolicy, RecallBudget, ScheduledJob, StoreTurnRecall, TurnRecall, World,
};
use aelio_agent::storage::{AelioStore, CompareSwap, LogicalTable, StoredRecord};
use aelio_agent::tenant::{PolicyAction, PolicyEffect, PolicySpec, PolicySubject};
use aelio_agent::ReasonCode;
use aelio_db_query::Database;
use std::sync::{Arc, Barrier};

fn store(tag: &str) -> AelioStore {
    let path = std::env::temp_dir().join(format!("aelio_recall_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let mut store = AelioStore::new(Database::create(path).unwrap(), 8).unwrap();
    store.migrate_tenant("tenant-1", 0).unwrap();
    store
}

#[test]
fn deep_turn_recall_is_bounded_grounded_and_user_scoped() {
    let embedder = HashEmbedder::new(8).unwrap();
    let shared = store("turn_recall");
    let mut memories = MemoryStore::new("tenant-1", shared.clone());
    memories
        .remember(
            "user-1",
            Memory {
                id: "seat-preference".into(),
                kind: MemoryKind::Factual,
                text: "The customer prefers an aisle seat".into(),
                provenance: MemoryProvenance {
                    source_kind: "conversation".into(),
                    source_id: "turn-old".into(),
                    observed_at_ms: 1,
                    actor: "user-1".into(),
                },
                confidence: 1.0,
                valid_time: ValidTime {
                    from_ms: 0,
                    to_ms: None,
                },
                expires_at_ms: None,
                decay: DecayPolicy {
                    half_life_ms: None,
                    floor: 1.0,
                },
                entity_id: None,
                open_loop_state: None,
                procedure_version: None,
            },
            &embedder,
            1,
        )
        .unwrap();

    let mut runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), shared).unwrap();
    let result = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "recall-turn".into(),
            user_id: "user-1".into(),
            utterance: "what travel seat preference did I tell you previously?".into(),
        })
        .unwrap();
    assert!(result
        .steps
        .iter()
        .any(|step| { step.name == "Recall.Assemble" && step.detail.contains("claims=1") }));
    assert!(result.steps.iter().any(|step| step.name == "Recall.Answer"));
    assert!(result.reply.text.contains("aisle seat"));
    assert!(result
        .reply
        .claim_refs
        .iter()
        .any(|claim| claim.starts_with("memory.seat-preference")));
}

#[test]
fn explicit_memory_round_trip_is_durable_scoped_and_model_free_on_write() {
    let shared = store("explicit_memory_round_trip");
    let mut runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), shared.clone()).unwrap();

    let stored = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "remember-turn".into(),
            user_id: "user-1".into(),
            utterance: "remember that I prefer window seats".into(),
        })
        .unwrap();
    assert_eq!(stored.llm_calls, 0);
    assert!(stored
        .steps
        .iter()
        .any(|step| step.name == "Memory.Request"));
    assert!(stored
        .steps
        .iter()
        .any(|step| { step.name == "Memory.Store" && step.detail.contains("stored explicit") }));

    // Reconstructing the runtime proves this is storage-backed rather than process memory.
    drop(runtime);
    let mut restarted =
        DurableRuntime::new(World::demo_tenant("tenant-1"), shared.clone()).unwrap();
    let recalled = restarted
        .run_turn(DurableTurnRequest {
            turn_id: "recall-explicit-turn".into(),
            user_id: "user-1".into(),
            utterance: "what seat preference did I tell you previously?".into(),
        })
        .unwrap();
    assert!(recalled
        .steps
        .iter()
        .any(|step| step.name == "Recall.Answer"));
    assert!(recalled.reply.text.contains("window seats"));
    assert!(recalled
        .reply
        .claim_refs
        .iter()
        .any(|claim| claim.starts_with("memory.explicit.")));

    let other_user = restarted
        .run_turn(DurableTurnRequest {
            turn_id: "recall-other-user-turn".into(),
            user_id: "user-2".into(),
            utterance: "what seat preference did I tell you previously?".into(),
        })
        .unwrap();
    assert!(!other_user.reply.text.contains("window seats"));
    assert!(!other_user
        .reply
        .claim_refs
        .iter()
        .any(|claim| claim.starts_with("memory.explicit.")));

    let forgotten = restarted
        .run_turn(DurableTurnRequest {
            turn_id: "forget-explicit-turn".into(),
            user_id: "user-1".into(),
            utterance: "forget that I prefer window seats".into(),
        })
        .unwrap();
    assert_eq!(forgotten.llm_calls, 0);
    assert!(forgotten
        .steps
        .iter()
        .any(|step| step.name == "Memory.Forget"));

    drop(restarted);
    let mut after_forget = DurableRuntime::new(World::demo_tenant("tenant-1"), shared).unwrap();
    let absent = after_forget
        .run_turn(DurableTurnRequest {
            turn_id: "recall-after-forget-turn".into(),
            user_id: "user-1".into(),
            utterance: "what seat preference did I tell you previously?".into(),
        })
        .unwrap();
    assert!(!absent.reply.text.contains("window seats"));
    assert!(!absent.steps.iter().any(|step| step.name == "Recall.Answer"));
}

#[test]
fn explicit_memory_rejects_secret_like_values_without_using_a_model() {
    let shared = store("explicit_memory_secret");
    let mut runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), shared.clone()).unwrap();
    let result = runtime
        .run_turn(DurableTurnRequest {
            turn_id: "secret-memory-turn".into(),
            user_id: "user-1".into(),
            utterance: "remember that my login code is 123456".into(),
        })
        .unwrap();
    assert_eq!(result.llm_calls, 0);
    assert!(result
        .steps
        .iter()
        .any(|step| { step.name == "Memory.Store" && step.detail.contains("rejected") }));
    assert!(shared
        .list::<Memory>("tenant-1", LogicalTable::Memories, Some("active"), 10)
        .unwrap()
        .is_empty());
}

fn document_write_policy() -> Vec<PolicySpec> {
    vec![PolicySpec {
        id: "allow-document-test".into(),
        effect: PolicyEffect::Allow,
        subject: PolicySubject::default(),
        action: PolicyAction::default(),
        condition: Predicate::True,
        reason_code: "test".into(),
        priority: 1,
    }]
}

#[test]
fn memory_taxonomy_is_user_scoped_valid_time_and_expiry_aware() {
    let embedder = HashEmbedder::new(8).unwrap();
    let mut memories = MemoryStore::new("tenant-1", store("memory"));
    memories
        .remember(
            "user-1",
            Memory {
                id: "fact-1".into(),
                kind: MemoryKind::Factual,
                text: "prefers window seats".into(),
                provenance: MemoryProvenance {
                    source_kind: "turn".into(),
                    source_id: "turn-1".into(),
                    observed_at_ms: 10,
                    actor: "user".into(),
                },
                confidence: 0.9,
                valid_time: ValidTime {
                    from_ms: 10,
                    to_ms: Some(100),
                },
                expires_at_ms: Some(90),
                decay: DecayPolicy {
                    half_life_ms: Some(1_000),
                    floor: 0.2,
                },
                entity_id: None,
                open_loop_state: None,
                procedure_version: None,
            },
            &embedder,
            10,
        )
        .unwrap();
    assert!(matches!(
        memories.exact("user-1", "fact-1", 20).unwrap(),
        Some((MemoryShape::Factual { .. }, _))
    ));
    assert!(memories.exact("user-2", "fact-1", 20).unwrap().is_none());
    assert!(memories.exact("user-1", "fact-1", 90).unwrap().is_none());
}

#[test]
fn live_recall_enforces_hit_and_character_budgets() {
    let shared = store("recall_hard_budgets");
    let embedder = HashEmbedder::new(8).unwrap();
    let mut memories = MemoryStore::new("tenant-1", shared.clone());
    for index in 0..12 {
        memories
            .remember(
                "user-1",
                Memory {
                    id: format!("preference-{index}"),
                    kind: MemoryKind::Factual,
                    text: format!("travel preference {index} is a window seat near the front"),
                    provenance: MemoryProvenance {
                        source_kind: "test".into(),
                        source_id: format!("turn-{index}"),
                        observed_at_ms: index,
                        actor: "user-1".into(),
                    },
                    confidence: 1.0,
                    valid_time: ValidTime {
                        from_ms: 0,
                        to_ms: None,
                    },
                    expires_at_ms: None,
                    decay: DecayPolicy {
                        half_life_ms: None,
                        floor: 1.0,
                    },
                    entity_id: None,
                    open_loop_state: None,
                    procedure_version: None,
                },
                &embedder,
                index,
            )
            .unwrap();
    }
    let mut recall = StoreTurnRecall::new("tenant-1", shared).unwrap();
    let evidence = recall
        .retrieve(
            "user-1",
            "travel window seat preference",
            100,
            RecallBudget {
                max_hits: 3,
                max_chars: 120,
            },
        )
        .unwrap();
    assert!(evidence.claims.len() <= 3);
    let chars: usize = evidence
        .claims
        .iter()
        .map(|claim| {
            aelio_agent::ops::pure::value_to_json(&claim.value)
                .to_string()
                .chars()
                .count()
        })
        .sum();
    assert!(chars <= 120);
    assert!(evidence.truncated);
}

#[test]
fn virtual_documents_reject_paths_and_patch_sections_with_cas() {
    let embedder = HashEmbedder::new(8).unwrap();
    let shared = store("documents");
    let mut documents =
        VirtualDocumentStore::new("tenant-1", shared.clone(), DocumentQuota::default()).unwrap();
    let policies = document_write_policy();
    let policy = PolicyCtx::default();
    let context = DocumentMutationContext {
        policies: &policies,
        policy: &policy,
        embedder: &embedder,
        now_ms: 1,
    };
    let invalid = VirtualDocument {
        id: "../secret".into(),
        namespace: DocumentNamespace::User("user-1".into()),
        title: "bad".into(),
        sections: vec![DocumentSection {
            id: "main".into(),
            text: "hidden".into(),
        }],
        revision: 0,
    };
    assert_eq!(
        documents.write(invalid, &context).unwrap_err().code,
        ReasonCode::Validation
    );

    let namespace = DocumentNamespace::User("user-1".into());
    documents
        .write(
            VirtualDocument {
                id: "guide".into(),
                namespace: namespace.clone(),
                title: "Travel".into(),
                sections: vec![DocumentSection {
                    id: "booking".into(),
                    text: "Book a window seat".into(),
                }],
                revision: 0,
            },
            &DocumentMutationContext {
                now_ms: 2,
                ..context
            },
        )
        .unwrap();
    let stored: StoredRecord<VirtualDocument> = shared
        .get("tenant-1", LogicalTable::Documents, "user:user-1:guide")
        .unwrap()
        .unwrap();
    assert!(matches!(
        documents
            .patch_section(
                &namespace,
                "guide",
                DocumentSection {
                    id: "booking".into(),
                    text: "Book an aisle seat".into(),
                },
                stored.version,
                &DocumentMutationContext {
                    now_ms: 3,
                    ..context
                },
            )
            .unwrap(),
        CompareSwap::Updated { .. }
    ));
    assert_eq!(
        documents
            .section(&namespace, "guide", "booking")
            .unwrap()
            .unwrap()
            .text,
        "Book an aisle seat"
    );
}

#[test]
fn proactive_gates_run_before_single_durable_enqueue() {
    let shared = store("proactive");
    let mut proactive = ProactiveLoop::new("tenant-1", shared.clone());
    let policy = ProactivePolicy {
        min_cadence_ms: 1_000,
        max_enqueues_per_day: 1,
        suppression_ms: 500,
        max_job_attempts: 2,
    };
    let mut candidate = ProactiveCandidate {
        user_id: "user-1".into(),
        fingerprint: "open-loop-1".into(),
        payload: serde_json::json!({"message": "follow up"}),
        opted_in: false,
        deterministic_gate: true,
        confidence_millis: 900,
        min_confidence_millis: 800,
    };
    assert!(matches!(
        proactive
            .evaluate_and_enqueue(candidate.clone(), &policy, 10_000)
            .unwrap(),
        ProactiveDecision::Suppressed { .. }
    ));
    let jobs: Vec<StoredRecord<ScheduledJob>> = shared
        .list("tenant-1", LogicalTable::Jobs, None, 10)
        .unwrap();
    assert!(
        jobs.is_empty(),
        "failed deterministic gates enqueue nothing"
    );

    candidate.opted_in = true;
    assert!(matches!(
        proactive
            .evaluate_and_enqueue(candidate.clone(), &policy, 10_000)
            .unwrap(),
        ProactiveDecision::Enqueued { .. }
    ));
    assert!(matches!(
        proactive
            .evaluate_and_enqueue(candidate, &policy, 10_000)
            .unwrap(),
        ProactiveDecision::Suppressed { .. }
    ));
    let jobs: Vec<StoredRecord<ScheduledJob>> = shared
        .list("tenant-1", LogicalTable::Jobs, None, 10)
        .unwrap();
    assert_eq!(jobs.len(), 1);
}

#[test]
fn concurrent_proactive_candidates_cannot_exceed_the_daily_cap() {
    let shared = store("proactive_race");
    let policy = ProactivePolicy {
        min_cadence_ms: 1_000,
        max_enqueues_per_day: 1,
        suppression_ms: 0,
        max_job_attempts: 2,
    };
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|index| {
            let store = shared.clone();
            let policy = policy.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut proactive = ProactiveLoop::new("tenant-1", store);
                barrier.wait();
                proactive.evaluate_and_enqueue(
                    ProactiveCandidate {
                        user_id: "same-user".into(),
                        fingerprint: format!("candidate-{index}"),
                        payload: serde_json::json!({"index": index}),
                        opted_in: true,
                        deterministic_gate: true,
                        confidence_millis: 900,
                        min_confidence_millis: 800,
                    },
                    &policy,
                    10_000,
                )
            })
        })
        .collect();
    let outcomes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    // Depending on the valid interleaving, the loser either collides while reserving the state
    // (Conflict) or reads the winner's committed reservation and is deterministically Suppressed.
    // The invariant is one enqueue/job, not one `Ok` result.
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, Ok(ProactiveDecision::Enqueued { .. })))
            .count(),
        1
    );
    assert!(outcomes.iter().any(|result| {
        matches!(result, Ok(ProactiveDecision::Suppressed { .. }))
            || result
                .as_ref()
                .is_err_and(|error| error.code == ReasonCode::Conflict)
    }));
    let jobs: Vec<StoredRecord<ScheduledJob>> = shared
        .list("tenant-1", LogicalTable::Jobs, None, 10)
        .unwrap();
    assert_eq!(jobs.len(), 1);
}
