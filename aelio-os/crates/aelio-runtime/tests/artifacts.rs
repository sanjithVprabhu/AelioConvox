use aelio_runtime::{
    Artifact, ArtifactActor, ArtifactClass, ArtifactEffect, ArtifactError, ArtifactInput,
    ArtifactInterface, ArtifactPins, ArtifactRepository, ArtifactStatus, ArtifactTier,
    ArtifactTransition, ArtifactTrigger, BuildBudget, BuildExample, BuildJobRepository,
    BuildPolicy, BuildScope, BuildSpec, BuildSpecDraft, BuildStage, CapabilityReason,
    CapabilityRequestDraft, CapabilityRequestRepository, ImprintDeclaration, ImprintField,
    ImprintRegistry, ImprintSensitivity, ImprintType, LeaseRequest, LeaseStatus,
    NameLeaseRepository, Provenance, Runtime, RuntimeConfig, VerificationCacheEntry,
    VerificationCacheRepository, VerificationVerdict, ARTIFACT_TABLE, DEFAULT_QUEUE_DEPTH,
    REPOSITORY_SCHEMA_TABLE, REPOSITORY_SCHEMA_TENANT,
};
use aelio_sol::SolValue;
use aelio_store::{EmbeddedStore, MemoryStore, Store};
use std::sync::{Arc, Barrier};

fn artifact(id: &str, body: serde_json::Value) -> Artifact {
    Artifact::new(
        id,
        1,
        ArtifactClass::Flow,
        ArtifactTier::Auto,
        "1",
        env!("CARGO_PKG_VERSION"),
        ArtifactInterface {
            inputs: vec![ArtifactInput {
                name: "input".into(),
                imprint: "message.input@1".into(),
                required: true,
                sensitivity: "internal".into(),
            }],
            output: "message.output@1".into(),
        },
        "test artifact",
        vec!["conversation".into()],
        vec![ArtifactEffect::Pure],
        ArtifactPins {
            prompts: vec!["prompt.reply@1".into()],
            ..ArtifactPins::default()
        },
        vec![serde_json::json!({"input": "hi", "output": "hello"})],
        body,
        Provenance {
            built_by: Some("root.harness@1".into()),
            ..Provenance::default()
        },
    )
    .unwrap()
}

#[test]
fn identity_is_canonical_and_excludes_authoring_metadata() {
    let original = artifact("flow.greeting", serde_json::json!({"op": "Identity"}));
    let mut metadata_only = original.clone();
    metadata_only.description = "better search description".into();
    metadata_only.tags = vec!["updated".into()];
    metadata_only.provenance.requester = Some("another builder".into());
    assert_eq!(
        original.compute_hash().unwrap(),
        metadata_only.compute_hash().unwrap()
    );

    let changed = artifact("flow.greeting", serde_json::json!({"op": "Const"}));
    assert_ne!(original.hash, changed.hash);
    let mut changed_pin = original.clone();
    changed_pin.pins.prompts = vec!["prompt.reply@2".into()];
    assert_ne!(original.hash, changed_pin.compute_hash().unwrap());
}

#[test]
fn declared_imprints_resolve_nested_shapes_and_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let store = EmbeddedStore::open(directory.path()).unwrap();
    let mut artifacts = ArtifactRepository::open(store.clone()).unwrap();
    let child = ImprintDeclaration {
        id: "test.address".into(),
        version: 1,
        required: vec![ImprintField {
            name: "city".into(),
            ty: ImprintType::Str,
            sensitivity: None,
        }],
        optional: vec![],
        open: false,
        default_sensitivity: ImprintSensitivity::Internal,
    };
    let parent = ImprintDeclaration {
        id: "test.customer".into(),
        version: 1,
        required: vec![
            ImprintField {
                name: "id".into(),
                ty: ImprintType::Int,
                sensitivity: None,
            },
            ImprintField {
                name: "address".into(),
                ty: ImprintType::Sol {
                    imprint: "test.address@1".into(),
                },
                sensitivity: Some(ImprintSensitivity::Pii),
            },
        ],
        optional: vec![],
        open: false,
        default_sensitivity: ImprintSensitivity::Internal,
    };
    for declaration in [&child, &parent] {
        artifacts
            .put_vendor_promoted("aelio.vendor", declaration.as_vendor_artifact().unwrap())
            .unwrap();
    }
    let registry = ImprintRegistry::open(store).unwrap();
    registry
        .validate_value(
            "tenant-a",
            "test.customer@1",
            &serde_json::json!({"id":7,"address":{"city":"Pune"}}),
        )
        .unwrap();
    assert!(registry
        .validate_value(
            "tenant-a",
            "test.customer@1",
            &serde_json::json!({"id":"7","address":{"city":"Pune"}}),
        )
        .is_err());
    assert!(registry
        .validate_value(
            "tenant-a",
            "test.customer@1",
            &serde_json::json!({"id":7,"address":{"city":"Pune","secret":true}}),
        )
        .is_err());
    assert!(registry.resolve("tenant-a", "test.missing@1").is_err());
}

#[test]
fn builder_v2_imprints_are_closed_bounded_and_validate_nested_topology() {
    let directory = tempfile::tempdir().unwrap();
    let _runtime = Runtime::open(RuntimeConfig {
        data_dir: directory.path().into(),
        host_url: None,
        host_token: None,
        event_key_secret: [41; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();
    let registry = ImprintRegistry::open(EmbeddedStore::open(directory.path()).unwrap()).unwrap();

    let selection = serde_json::json!({
        "selected":[{"id":"flow.seed@1","score":0.91,"role":"source"}],
        "runners_up":[],
        "unmet":[],
        "undeterminable":false
    });
    registry
        .validate_value("tenant-a", "aelio.selection_result@2", &selection)
        .unwrap();
    let mut unknown = selection.clone();
    unknown["model_secret"] = serde_json::json!(true);
    assert!(registry
        .validate_value("tenant-a", "aelio.selection_result@2", &unknown)
        .unwrap_err()
        .to_string()
        .contains("rejects key"));

    let composition = serde_json::json!({
        "tree":[{"nid":"seed","artifact":"flow.seed@1"}],
        "seams":[
            {"from":{"kind":"parent_input","slot":"turn"},"to":{"kind":"node_input","node":"seed","slot":"turn"}},
            {"from":{"kind":"node_output","node":"seed"},"to":{"kind":"parent_output"}}
        ],
        "undeterminable":false
    });
    registry
        .validate_value("tenant-a", "aelio.compose_result@2", &composition)
        .unwrap();
    let mut invented = composition.clone();
    invented["tree"][0]["executable_program"] = serde_json::json!({"op":"Call"});
    assert!(registry
        .validate_value("tenant-a", "aelio.compose_result@2", &invented)
        .is_err());
    let mut malformed_seam = composition;
    malformed_seam["seams"][0]["from"]["unknown"] = serde_json::json!(1);
    assert!(registry
        .validate_value("tenant-a", "aelio.compose_result@2", &malformed_seam)
        .is_err());
}

#[test]
fn rejects_unpinned_refs_duplicate_inputs_and_forged_hashes() {
    let mut unpinned = artifact("flow.bad", serde_json::json!({}));
    unpinned.interface.output = "message.output".into();
    assert!(matches!(
        unpinned.validate(),
        Err(ArtifactError::Invalid(_))
    ));

    let mut duplicate = artifact("flow.bad", serde_json::json!({}));
    duplicate
        .interface
        .inputs
        .push(duplicate.interface.inputs[0].clone());
    assert!(matches!(
        duplicate.validate(),
        Err(ArtifactError::Invalid(_))
    ));

    let mut forged = artifact("flow.bad", serde_json::json!({}));
    forged.hash = "0".repeat(64);
    assert!(matches!(
        forged.validate(),
        Err(ArtifactError::HashMismatch { .. })
    ));
}

#[test]
fn immutable_insert_is_idempotent_but_rejects_same_version_with_new_content() {
    let mut repository = ArtifactRepository::open(MemoryStore::new()).unwrap();
    let first = artifact("flow.greeting", serde_json::json!({"op": "Identity"}));
    repository
        .put_proposed("tenant-a", first.clone(), ArtifactActor::System)
        .unwrap();
    repository
        .put_proposed("tenant-a", first, ArtifactActor::System)
        .unwrap();

    let collision = artifact("flow.greeting", serde_json::json!({"op": "Const"}));
    assert!(matches!(
        repository.put_proposed("tenant-a", collision, ArtifactActor::System),
        Err(ArtifactError::Conflict(_))
    ));
}

#[test]
fn tenant_namespaces_are_isolated() {
    let mut repository = ArtifactRepository::open(MemoryStore::new()).unwrap();
    repository
        .put_proposed(
            "tenant-a",
            artifact("flow.greeting", serde_json::json!({"tenant": "a"})),
            ArtifactActor::System,
        )
        .unwrap();
    repository
        .put_proposed(
            "tenant-b",
            artifact("flow.greeting", serde_json::json!({"tenant": "b"})),
            ArtifactActor::System,
        )
        .unwrap();
    assert_ne!(
        repository
            .get("tenant-a", "flow.greeting", 1)
            .unwrap()
            .unwrap()
            .artifact
            .hash,
        repository
            .get("tenant-b", "flow.greeting", 1)
            .unwrap()
            .unwrap()
            .artifact
            .hash
    );
    assert!(repository
        .get("tenant-c", "flow.greeting", 1)
        .unwrap()
        .is_none());
}

#[test]
fn lifecycle_enforces_thresholds_actors_and_legal_transitions() {
    let mut repository = ArtifactRepository::open(MemoryStore::new()).unwrap();
    repository
        .put_proposed(
            "tenant-a",
            artifact("flow.greeting", serde_json::json!({})),
            ArtifactActor::System,
        )
        .unwrap();
    let shadow = repository
        .transition(
            "tenant-a",
            "flow.greeting",
            1,
            ArtifactTransition {
                expected: ArtifactStatus::Proposed,
                trigger: ArtifactTrigger::StructuralPass,
                actor: ArtifactActor::System,
                evidence_snapshot_hash: None,
            },
        )
        .unwrap();
    assert_eq!(shadow.status, ArtifactStatus::Shadow);
    assert_eq!(shadow.history.len(), 2);

    assert!(matches!(
        repository.transition(
            "tenant-a",
            "flow.greeting",
            1,
            ArtifactTransition {
                expected: ArtifactStatus::Shadow,
                trigger: ArtifactTrigger::ShadowThresholdsMet {
                    distinct_inputs: 19,
                    validation_rate: 1.0,
                    approved: true,
                },
                actor: ArtifactActor::System,
                evidence_snapshot_hash: None,
            },
        ),
        Err(ArtifactError::Invalid(_))
    ));
    assert!(matches!(
        repository.transition(
            "tenant-a",
            "flow.greeting",
            1,
            ArtifactTransition {
                expected: ArtifactStatus::Shadow,
                trigger: ArtifactTrigger::RetireManual,
                actor: ArtifactActor::System,
                evidence_snapshot_hash: None,
            },
        ),
        Err(ArtifactError::IllegalTransition(_))
    ));
}

#[test]
fn lifecycle_and_history_survive_database_restart_atomically() {
    let directory = tempfile::tempdir().unwrap();
    {
        let store = EmbeddedStore::open(directory.path()).unwrap();
        let mut repository = ArtifactRepository::open(store).unwrap();
        repository
            .put_proposed(
                "tenant-a",
                artifact("flow.restart", serde_json::json!({})),
                ArtifactActor::System,
            )
            .unwrap();
        repository
            .transition(
                "tenant-a",
                "flow.restart",
                1,
                ArtifactTransition {
                    expected: ArtifactStatus::Proposed,
                    trigger: ArtifactTrigger::StructuralPass,
                    actor: ArtifactActor::System,
                    evidence_snapshot_hash: Some("a".repeat(64)),
                },
            )
            .unwrap();
    }
    let store = EmbeddedStore::open(directory.path()).unwrap();
    let repository = ArtifactRepository::open(store).unwrap();
    let restored = repository
        .get("tenant-a", "flow.restart", 1)
        .unwrap()
        .unwrap();
    assert_eq!(restored.status, ArtifactStatus::Shadow);
    assert_eq!(restored.history.len(), 2);
    assert_eq!(restored.history[1].from, Some(ArtifactStatus::Proposed));
    assert_eq!(
        restored.history[1].evidence_snapshot_hash,
        Some("a".repeat(64))
    );
}

#[test]
fn concurrent_transition_has_one_cas_winner() {
    let store = MemoryStore::new();
    let mut setup = ArtifactRepository::open(store.clone()).unwrap();
    setup
        .put_proposed(
            "tenant-a",
            artifact("flow.race", serde_json::json!({})),
            ArtifactActor::System,
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let mut repository = ArtifactRepository::open(store.clone()).unwrap();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                repository.transition(
                    "tenant-a",
                    "flow.race",
                    1,
                    ArtifactTransition {
                        expected: ArtifactStatus::Proposed,
                        trigger: ArtifactTrigger::StructuralPass,
                        actor: ArtifactActor::System,
                        evidence_snapshot_hash: None,
                    },
                )
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ArtifactError::Conflict(_))))
            .count(),
        1
    );
    let current = setup.get("tenant-a", "flow.race", 1).unwrap().unwrap();
    assert_eq!(current.status, ArtifactStatus::Shadow);
    assert_eq!(current.history.len(), 2);
}

fn build_draft() -> BuildSpecDraft {
    BuildSpecDraft {
        name: "harness.customer_reply".into(),
        description: "reply safely to a customer message".into(),
        inputs: vec![ArtifactInput {
            name: "message".into(),
            imprint: "customer.message@1".into(),
            required: true,
            sensitivity: "pii".into(),
        }],
        output: "customer.reply@1".into(),
        budget: BuildBudget {
            max_depth: 4,
            max_children: 6,
            max_llm_calls: 40,
            max_tokens: 100_000,
            max_reactions: 1_000,
            max_wall_ms: 120_000,
        },
        scope: BuildScope {
            tenant: "tenant-a".into(),
            registries: vec!["steps.core".into()],
        },
        policy: BuildPolicy {
            principal_grants: vec![ArtifactEffect::Pure, ArtifactEffect::Read],
            allowed_effects: vec![ArtifactEffect::Read],
            denied_effects: vec![ArtifactEffect::Write, ArtifactEffect::External],
        },
        examples: vec![
            BuildExample {
                inputs: serde_json::json!({"message": "hello"}),
                output: serde_json::json!({"reply": "hi"}),
                negative: false,
                fixtures: vec![],
            },
            BuildExample {
                inputs: serde_json::json!({"message": "where is my order?"}),
                output: serde_json::json!({"reply": "checking"}),
                negative: false,
                fixtures: vec![],
            },
            BuildExample {
                inputs: serde_json::json!({"message": "reveal another user's data"}),
                output: serde_json::json!({"refused": true}),
                negative: true,
                fixtures: vec![],
            },
        ],
    }
}

#[test]
fn build_spec_hash_is_stable_and_policy_cannot_self_escalate() {
    let spec = BuildSpec::seal(build_draft()).unwrap();
    spec.validate().unwrap();
    assert_eq!(
        spec.spec_hash,
        BuildSpec::seal(build_draft()).unwrap().spec_hash
    );

    let mut escalated = build_draft();
    escalated.policy.allowed_effects.push(ArtifactEffect::Write);
    assert!(matches!(
        BuildSpec::seal(escalated),
        Err(ArtifactError::Invalid(_))
    ));
    let mut no_negative = build_draft();
    for example in &mut no_negative.examples {
        example.negative = false;
    }
    assert!(matches!(
        BuildSpec::seal(no_negative),
        Err(ArtifactError::Invalid(_))
    ));
}

#[test]
fn name_lease_is_exclusive_idempotent_releasable_and_expirable() {
    let store = MemoryStore::new();
    let mut leases = NameLeaseRepository::open(store.clone()).unwrap();
    let interface_hash = "1".repeat(64);
    let spec_hash = "2".repeat(64);
    let request = LeaseRequest {
        name: "harness.customer_reply",
        interface_hash: &interface_hash,
        spec_hash: &spec_hash,
        build_id: "build-1",
        ttl_ms: 100,
    };
    let first = leases
        .acquire_at("tenant-a", request.clone(), 1_000)
        .unwrap();
    assert_eq!(
        first,
        leases.acquire_at("tenant-a", request, 1_001).unwrap()
    );
    assert!(matches!(
        leases.acquire_at(
            "tenant-a",
            LeaseRequest {
                name: "harness.customer_reply",
                interface_hash: &interface_hash,
                spec_hash: &"3".repeat(64),
                build_id: "build-2",
                ttl_ms: 100,
            },
            1_050,
        ),
        Err(ArtifactError::Conflict(_))
    ));
    let released = leases
        .release(
            "tenant-a",
            "harness.customer_reply",
            &interface_hash,
            "build-1",
        )
        .unwrap();
    assert_eq!(released.status, LeaseStatus::Released);
    leases
        .acquire_at(
            "tenant-a",
            LeaseRequest {
                name: "harness.customer_reply",
                interface_hash: &interface_hash,
                spec_hash: &"3".repeat(64),
                build_id: "build-2",
                ttl_ms: 100,
            },
            1_051,
        )
        .unwrap();
    // The same takeover also works after expiry without an explicit release.
    leases
        .acquire_at(
            "tenant-a",
            LeaseRequest {
                name: "harness.customer_reply",
                interface_hash: &interface_hash,
                spec_hash: &"4".repeat(64),
                build_id: "build-3",
                ttl_ms: 100,
            },
            1_152,
        )
        .unwrap();
}

#[test]
fn verification_cache_is_immutable_tenant_scoped_and_restart_durable() {
    let directory = tempfile::tempdir().unwrap();
    let entry = VerificationCacheEntry {
        candidate_hash: "a".repeat(64),
        examples_hash: "b".repeat(64),
        pin_context_hash: "c".repeat(64),
        verdict: VerificationVerdict::Pass,
        ledger_ref: "ledger:gate-1".into(),
    };
    {
        let store = EmbeddedStore::open(directory.path()).unwrap();
        let mut cache = VerificationCacheRepository::open(store).unwrap();
        cache.put("tenant-a", entry.clone()).unwrap();
        cache.put("tenant-a", entry.clone()).unwrap();
        let mut conflict = entry.clone();
        conflict.verdict = VerificationVerdict::Fail;
        assert!(matches!(
            cache.put("tenant-a", conflict),
            Err(ArtifactError::Conflict(_))
        ));
        assert!(cache
            .get(
                "tenant-b",
                &entry.candidate_hash,
                &entry.examples_hash,
                &entry.pin_context_hash,
            )
            .unwrap()
            .is_none());
    }
    let store = EmbeddedStore::open(directory.path()).unwrap();
    let cache = VerificationCacheRepository::open(store).unwrap();
    assert_eq!(
        cache
            .get(
                "tenant-a",
                &entry.candidate_hash,
                &entry.examples_hash,
                &entry.pin_context_hash,
            )
            .unwrap(),
        Some(entry)
    );
}

#[test]
fn repository_refuses_unknown_future_schema_instead_of_guessing() {
    let mut store = MemoryStore::new();
    store
        .put_if_absent(
            REPOSITORY_SCHEMA_TENANT,
            REPOSITORY_SCHEMA_TABLE,
            ARTIFACT_TABLE,
            SolValue::map([
                ("component", SolValue::str(ARTIFACT_TABLE)),
                ("schema_version", SolValue::Int(2)),
                ("min_reader_version", SolValue::Int(2)),
            ]),
        )
        .unwrap();
    assert!(matches!(
        ArtifactRepository::open(store),
        Err(ArtifactError::Invalid(detail)) if detail.contains("newer reader")
    ));
}

#[test]
fn gate_exempt_vendor_install_is_namespace_and_provenance_restricted() {
    let mut repository = ArtifactRepository::open(MemoryStore::new()).unwrap();
    let mut candidate = artifact("vendor.stock", serde_json::json!({"op":"Identity"}));
    candidate
        .provenance
        .metadata
        .insert("origin".into(), serde_json::json!("vendor"));
    assert!(matches!(
        repository.put_vendor_promoted("tenant-a", candidate.clone()),
        Err(ArtifactError::Invalid(_))
    ));
    candidate.provenance.metadata.clear();
    assert!(matches!(
        repository.put_vendor_promoted("aelio.vendor", candidate.clone()),
        Err(ArtifactError::Invalid(_))
    ));
    candidate
        .provenance
        .metadata
        .insert("origin".into(), serde_json::json!("vendor"));
    let record = repository
        .put_vendor_promoted("aelio.vendor", candidate)
        .unwrap();
    assert_eq!(record.status, ArtifactStatus::Promoted);
    assert_eq!(record.history.len(), 2);
    assert_eq!(record.history[1].trigger, ArtifactTrigger::VendorInstall);
}

#[test]
fn build_job_is_durable_idempotent_and_each_stage_is_cas_owned() {
    let directory = tempfile::tempdir().unwrap();
    let spec = BuildSpec::seal(build_draft()).unwrap();
    let original;
    {
        let store = EmbeddedStore::open(directory.path()).unwrap();
        let mut jobs = BuildJobRepository::open(store).unwrap();
        original = jobs.create("tenant-a", spec.clone()).unwrap();
        let duplicate = jobs.create("tenant-a", spec).unwrap();
        assert_eq!(original.job.build_id, duplicate.job.build_id);
        assert_eq!(original.job.revision, duplicate.job.revision);
    }

    let store = EmbeddedStore::open(directory.path()).unwrap();
    let mut jobs = BuildJobRepository::open(store).unwrap();
    let restarted = jobs
        .get("tenant-a", &original.job.build_id)
        .unwrap()
        .unwrap();
    assert_eq!(restarted.job.stage, BuildStage::Admit);
    let stale = restarted.clone();
    let mut next = restarted.job.clone();
    next.stage = BuildStage::Resolve;
    next.ledger.push("build:admit:ok".into());
    let resolved = jobs.commit("tenant-a", restarted, next).unwrap();
    assert_eq!(resolved.job.revision, 2);
    assert_eq!(resolved.job.stage, BuildStage::Resolve);

    let mut competing = stale.job.clone();
    competing.stage = BuildStage::Resolve;
    competing.ledger.push("build:admit:other".into());
    assert!(matches!(
        jobs.commit("tenant-a", stale, competing),
        Err(ArtifactError::Conflict(_))
    ));

    let mut illegal = resolved.job.clone();
    illegal.stage = BuildStage::Gate;
    assert!(matches!(
        jobs.commit("tenant-a", resolved, illegal),
        Err(ArtifactError::Invalid(_))
    ));
}

#[test]
fn capability_demand_deduplicates_by_tenant_need_and_reason() {
    let mut requests = CapabilityRequestRepository::open(MemoryStore::new()).unwrap();
    let draft = CapabilityRequestDraft {
        normalized_need: "produce a typed refund decision".into(),
        inputs: vec![ArtifactInput {
            name: "turn".into(),
            imprint: "aelio.turn.input@1".into(),
            required: true,
            sensitivity: "internal".into(),
        }],
        output: "refund.decision@1".into(),
        allowed_effects: vec![ArtifactEffect::Read],
        requester: "steward".into(),
        reason: CapabilityReason::MissingCapability,
        evidence_refs: vec!["ledger:1".into()],
    };
    let first = requests.record("tenant-a", draft.clone()).unwrap();
    let second = requests.record("tenant-a", draft.clone()).unwrap();
    let isolated = requests.record("tenant-b", draft).unwrap();
    assert_eq!(first.request_id, second.request_id);
    assert_eq!(second.demand_count, 2);
    assert_eq!(isolated.demand_count, 1);
    let open = requests.list_open("tenant-a", 10).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].demand_count, 2);
    let queued = requests
        .queue("tenant-a", &first.request_id, "build-attempt-one")
        .unwrap();
    assert_eq!(queued.status, aelio_runtime::CapabilityStatus::Queued);
    assert_eq!(queued.build_attempts, ["build-attempt-one"]);
    assert_eq!(
        requests
            .queue("tenant-a", &first.request_id, "build-attempt-one")
            .unwrap()
            .build_attempts
            .len(),
        1,
        "queue retry must be idempotent"
    );
    assert!(requests
        .queue("tenant-a", &first.request_id, "build-redirection")
        .is_err());
    let failed = requests
        .fail("tenant-a", &first.request_id, "build-attempt-one")
        .unwrap();
    assert_eq!(failed.status, aelio_runtime::CapabilityStatus::Failed);
    let reopened = requests.record("tenant-a", failed.draft.clone()).unwrap();
    assert_eq!(reopened.status, aelio_runtime::CapabilityStatus::Open);
    let queued = requests
        .queue("tenant-a", &first.request_id, "build-attempt-two")
        .unwrap();
    assert_eq!(queued.build_attempts.len(), 2);
    let resolved = requests
        .resolve("tenant-a", &first.request_id, "build-attempt-two")
        .unwrap();
    assert_eq!(resolved.status, aelio_runtime::CapabilityStatus::Resolved);
}
