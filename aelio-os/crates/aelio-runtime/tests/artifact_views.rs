use aelio_learn::{ProcedureCandidate, TraceStep};
use aelio_runtime::{
    AgreementObservation, Artifact, ArtifactActor, ArtifactClass, ArtifactEffect, ArtifactError,
    ArtifactInput, ArtifactInterface, ArtifactPins, ArtifactRepository, ArtifactStatus,
    ArtifactTier, ConverterArtifactDraft, DatasetArtifactDraft, DatasetModality,
    GenericArtifactGate, LearnedProcedureRequest, PathwayArtifactDraft, PathwayPrototype,
    ProcedureArtifactDraft, ProcedureSignatureStep, ProcedureTraceObservation, Provenance, Runtime,
    RuntimeConfig, TurnReply, TurnSubmit, DEFAULT_QUEUE_DEPTH,
};
use aelio_store::EmbeddedStore;
use std::collections::BTreeMap;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn every_legacy_view_lowers_to_one_immutable_registry_record() {
    let pathway = PathwayArtifactDraft {
        id: "pathway.login".into(),
        version: 1,
        decision_point: "unauthenticated".into(),
        fallback_artifact: "flow.help@1".into(),
        instantiates_flow: true,
        embedding_model: "embed.default@1".into(),
        tau: 0.8,
        delta: 0.1,
        entropy_ceiling: 0.5,
        prototypes: vec![PathwayPrototype {
            artifact: "flow.login@1".into(),
            embedding_hash: HASH_A.into(),
        }],
        examples: vec![],
    }
    .lower(Provenance::default())
    .unwrap();
    assert_eq!(pathway.class, ArtifactClass::Pathway);
    pathway.validate().unwrap();

    let procedure = ProcedureArtifactDraft {
        id: "procedure.login".into(),
        version: 1,
        situation_hash: HASH_B.into(),
        signature: vec![
            ProcedureSignatureStep {
                call_id: "flow.login".into(),
                arg_shape: "{turn:str}".into(),
            },
            ProcedureSignatureStep {
                call_id: "flow.login".into(),
                arg_shape: "{turn:str}".into(),
            },
        ],
        implementation: "flow.login@1".into(),
        steps: vec!["flow.login@1".into(), "flow.login@1".into()],
        tool_dependencies: vec!["send_otp@1".into()],
        prompt_dependencies: vec!["prompt.login@1".into()],
        embedding_model: "embed.default@1".into(),
        effect: ArtifactEffect::External,
        examples: vec![],
    }
    .lower(Provenance::default())
    .unwrap();
    assert_eq!(procedure.class, ArtifactClass::Procedure);
    assert_eq!(procedure.pins.artifacts, vec!["flow.login@1"]);
    assert_eq!(procedure.tier, ArtifactTier::Reviewed);
    procedure.validate().unwrap();

    let dataset = DatasetArtifactDraft {
        id: "dataset.customers".into(),
        version: 1,
        collection: "customers".into(),
        schema_hash: HASH_A.into(),
        modalities: vec![DatasetModality::Get, DatasetModality::TopkVector],
        max_limit: 100,
    }
    .lower(Provenance::default())
    .unwrap();
    assert_eq!(dataset.class, ArtifactClass::Dataset);
    assert_eq!(dataset.tier, ArtifactTier::Locked);
    dataset.validate().unwrap();

    let converter = ConverterArtifactDraft {
        id: "converter.customer".into(),
        version: 1,
        from_imprint: "customer.raw@1".into(),
        to_imprint: "customer.normalized@1".into(),
        rules: serde_json::json!([{"op":"rename","from":"full_name","to":"name"}]),
        tier: ArtifactTier::Auto,
        examples: vec![],
    }
    .lower(Provenance::default())
    .unwrap();
    assert_eq!(converter.class, ArtifactClass::Glu);
    converter.validate().unwrap();
}

#[test]
fn raw_noncanonical_legacy_body_is_rejected_at_repository_boundary() {
    let result = Artifact::new(
        "procedure.unsafe",
        1,
        ArtifactClass::Procedure,
        ArtifactTier::Auto,
        "1",
        env!("CARGO_PKG_VERSION"),
        ArtifactInterface {
            inputs: vec![ArtifactInput {
                name: "turn".into(),
                imprint: "aelio.turn.input@1".into(),
                required: true,
                sensitivity: "internal".into(),
            }],
            output: "aelio.turn.output@1".into(),
        },
        "unsafe procedure",
        vec![],
        vec![ArtifactEffect::Pure],
        ArtifactPins::default(),
        vec![],
        serde_json::json!({"code":"run arbitrary bytes"}),
        Provenance::default(),
    );
    assert!(matches!(result, Err(ArtifactError::Invalid(_))));
}

#[test]
fn invalid_hygiene_unbounded_dataset_and_open_converter_fail_closed() {
    let bad_pathway = PathwayArtifactDraft {
        id: "pathway.bad".into(),
        version: 1,
        decision_point: "root".into(),
        fallback_artifact: "flow.fallback@1".into(),
        instantiates_flow: false,
        embedding_model: "embed.default@1".into(),
        tau: f64::NAN,
        delta: 0.1,
        entropy_ceiling: 0.5,
        prototypes: vec![PathwayPrototype {
            artifact: "flow.answer@1".into(),
            embedding_hash: HASH_A.into(),
        }],
        examples: vec![],
    };
    assert!(bad_pathway.lower(Provenance::default()).is_err());

    let unbounded = DatasetArtifactDraft {
        id: "dataset.bad".into(),
        version: 1,
        collection: "rows".into(),
        schema_hash: HASH_A.into(),
        modalities: vec![DatasetModality::Range],
        max_limit: 0,
    };
    assert!(unbounded.lower(Provenance::default()).is_err());

    let arbitrary = ConverterArtifactDraft {
        id: "converter.bad".into(),
        version: 1,
        from_imprint: "a@1".into(),
        to_imprint: "b@1".into(),
        rules: serde_json::json!([{"op":"exec","code":"evil"}]),
        tier: ArtifactTier::Auto,
        examples: vec![],
    };
    assert!(arbitrary.lower(Provenance::default()).is_err());
}

#[test]
fn typed_views_share_distinct_evidence_and_reviewed_approval_gate() {
    let directory = tempfile::tempdir().unwrap();
    let store = EmbeddedStore::open(directory.path()).unwrap();
    let mut origin = serde_json::Map::new();
    origin.insert("origin".into(), serde_json::json!("vendor"));
    let dependency = Artifact::new(
        "flow.cancel_order",
        1,
        ArtifactClass::Flow,
        ArtifactTier::Locked,
        "1",
        env!("CARGO_PKG_VERSION"),
        ArtifactInterface {
            inputs: vec![],
            output: "aelio.turn.output@1".into(),
        },
        "test vendor dependency",
        vec![],
        vec![ArtifactEffect::Pure],
        ArtifactPins::default(),
        vec![],
        serde_json::json!({
            "program":{"nid":"root","op":"Const","v":{}},
            "targets":[],
            "prompts":[]
        }),
        Provenance {
            metadata: origin,
            ..Provenance::default()
        },
    )
    .unwrap();
    ArtifactRepository::open(store.clone())
        .unwrap()
        .put_vendor_promoted("aelio.vendor", dependency)
        .unwrap();
    let procedure = ProcedureArtifactDraft {
        id: "procedure.effectful".into(),
        version: 1,
        situation_hash: HASH_A.into(),
        signature: vec![ProcedureSignatureStep {
            call_id: "flow.cancel_order".into(),
            arg_shape: "{order_id:str}".into(),
        }],
        implementation: "flow.cancel_order@1".into(),
        steps: vec!["flow.cancel_order@1".into()],
        tool_dependencies: vec!["cancel_order@1".into()],
        prompt_dependencies: vec![],
        embedding_model: "embed.default@1".into(),
        effect: ArtifactEffect::External,
        examples: vec![],
    }
    .lower(Provenance::default())
    .unwrap();
    ArtifactRepository::open(store.clone())
        .unwrap()
        .put_proposed("tenant-a", procedure, ArtifactActor::System)
        .unwrap();
    let observations = (0..20)
        .map(|index| AgreementObservation {
            input_hash: format!("{index:064x}"),
            agreed: true,
            downstream_success: true,
            guard_violation: false,
            ledger_hash: format!("{:064x}", index + 100),
        })
        .collect::<Vec<_>>();
    let gate = GenericArtifactGate::new(store.clone());
    let held = gate
        .gate_agreement(
            "tenant-a",
            "procedure.effectful",
            1,
            ArtifactClass::Procedure,
            &observations,
            None,
        )
        .unwrap();
    assert_eq!(held.record.status, ArtifactStatus::Shadow);

    // Replaying the same observations cannot inflate distinct evidence, but an identified deployer
    // can approve first consumption once the real threshold is present.
    let admitted = gate
        .gate_agreement(
            "tenant-a",
            "procedure.effectful",
            1,
            ArtifactClass::Procedure,
            &observations,
            Some("deployer-1".into()),
        )
        .unwrap();
    assert_eq!(admitted.evidence.distinct_inputs, 20);
    assert_eq!(admitted.record.status, ArtifactStatus::Canary);
}

#[test]
fn procedure_mining_closes_through_runtime_owned_shadow_comparison() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(RuntimeConfig {
        data_dir: directory.path().into(),
        host_url: None,
        host_token: None,
        event_key_secret: [41; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();
    let mut origin = serde_json::Map::new();
    origin.insert("origin".into(), serde_json::json!("vendor"));
    let dependency = Artifact::new(
        "flow.learned_step",
        1,
        ArtifactClass::Flow,
        ArtifactTier::Locked,
        "1",
        env!("CARGO_PKG_VERSION"),
        ArtifactInterface {
            inputs: vec![],
            output: "aelio.turn.output@1".into(),
        },
        "test admitted step",
        vec![],
        vec![ArtifactEffect::Pure],
        ArtifactPins::default(),
        vec![],
        serde_json::json!({
            "program":{"nid":"root","op":"Const","v":{"learned":true}},
            "targets":[],
            "prompts":[]
        }),
        Provenance {
            metadata: origin,
            ..Provenance::default()
        },
    )
    .unwrap();
    runtime
        .artifact_repository()
        .unwrap()
        .put_vendor_promoted("aelio.vendor", dependency)
        .unwrap();
    let signature = vec![TraceStep {
        call_id: "learned_step".into(),
        arg_shape: "{message:str}".into(),
    }];
    let proposed = runtime
        .propose_learned_procedure(LearnedProcedureRequest {
            tenant: "tenant-a".into(),
            id: "procedure.learned".into(),
            version: 1,
            situation_hash: HASH_A.into(),
            candidate: ProcedureCandidate {
                signature: signature.clone(),
                occurrences: 5,
                distinct_flows: 5,
            },
            implementation: "flow.learned_step@1".into(),
            call_artifacts: BTreeMap::from([("learned_step".into(), "flow.learned_step@1".into())]),
            tool_dependencies: vec![],
            prompt_dependencies: vec![],
            embedding_model: "embed.default@1".into(),
            effect: ArtifactEffect::Pure,
            examples: vec![],
            requester: "learner".into(),
        })
        .unwrap();
    assert_eq!(proposed.status, ArtifactStatus::Proposed);

    let traces = (0..20)
        .map(|index| ProcedureTraceObservation {
            input_hash: format!("{index:064x}"),
            actual: if index < 2 {
                vec![TraceStep {
                    call_id: "different".into(),
                    arg_shape: "{}".into(),
                }]
            } else {
                signature.clone()
            },
            downstream_success: true,
            guard_violation: false,
            ledger_hash: format!("{:064x}", index + 200),
        })
        .collect::<Vec<_>>();
    let held = runtime
        .gate_learned_procedure("tenant-a", "procedure.learned", 1, &traces, None)
        .unwrap();
    assert_eq!(held.evidence.validation_rate, 0.9);
    assert_eq!(held.record.status, ArtifactStatus::Shadow);

    let additional = (20..40)
        .map(|index| ProcedureTraceObservation {
            input_hash: format!("{index:064x}"),
            actual: signature.clone(),
            downstream_success: true,
            guard_violation: false,
            ledger_hash: format!("{:064x}", index + 200),
        })
        .collect::<Vec<_>>();
    let admitted = runtime
        .gate_learned_procedure("tenant-a", "procedure.learned", 1, &additional, None)
        .unwrap();
    assert_eq!(admitted.evidence.distinct_inputs, 40);
    assert_eq!(admitted.evidence.validation_rate, 0.95);
    assert_eq!(admitted.record.status, ArtifactStatus::Canary);
    let reply = runtime
        .invoke_pinned_artifact(TurnSubmit {
            tenant: "tenant-a".into(),
            instance_id: "learned-invocation-1".into(),
            flow_id: "procedure.learned".into(),
            flow_rev: "1".into(),
            input: serde_json::json!({"turn":{"message":"hello"}}),
        })
        .unwrap();
    assert!(matches!(
        reply,
        TurnReply::Completed { bag, .. } if bag == serde_json::json!({"learned":true})
    ));
}
