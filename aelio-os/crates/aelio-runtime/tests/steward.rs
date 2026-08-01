use aelio_runtime::{
    Artifact, ArtifactActor, ArtifactClass, ArtifactEffect, ArtifactInput, ArtifactInterface,
    ArtifactPins, ArtifactStatus, ArtifactTier, ArtifactTransition, ArtifactTrigger,
    CapabilityRequestRepository, PromotionProposalStatus, Provenance, Runtime, RuntimeConfig,
    DEFAULT_QUEUE_DEPTH,
};

fn runtime(path: &std::path::Path) -> Runtime {
    Runtime::open(RuntimeConfig {
        data_dir: path.into(),
        host_url: None,
        host_token: None,
        event_key_secret: [41; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap()
}

fn dependent() -> Artifact {
    Artifact::new(
        "flow.dependent",
        1,
        ArtifactClass::Flow,
        ArtifactTier::Auto,
        "1",
        "kernel@1",
        ArtifactInterface {
            inputs: vec![ArtifactInput {
                name: "turn".into(),
                imprint: "aelio.turn.input@1".into(),
                required: true,
                sensitivity: "internal".into(),
            }],
            output: "aelio.turn.output@1".into(),
        },
        "flow depending on a versioned tool",
        vec!["dependent".into()],
        vec![ArtifactEffect::Read],
        ArtifactPins {
            targets: vec!["tool.departing@1".into()],
            ..ArtifactPins::default()
        },
        vec![],
        serde_json::json!({"program":{"nid":"root","op":"Const","v":{}}}),
        Provenance::default(),
    )
    .unwrap()
}

#[test]
fn steward_demotes_dependency_consumers_and_only_proposes_promotion() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let mut artifacts = runtime.artifact_repository().unwrap();
    let artifact = dependent();
    artifacts
        .put_proposed("tenant-a", artifact.clone(), ArtifactActor::System)
        .unwrap();
    artifacts
        .transition(
            "tenant-a",
            &artifact.id,
            1,
            ArtifactTransition {
                expected: ArtifactStatus::Proposed,
                trigger: ArtifactTrigger::StructuralPass,
                actor: ArtifactActor::System,
                evidence_snapshot_hash: Some(artifact.hash.clone()),
            },
        )
        .unwrap();
    artifacts
        .transition(
            "tenant-a",
            &artifact.id,
            1,
            ArtifactTransition {
                expected: ArtifactStatus::Shadow,
                trigger: ArtifactTrigger::ShadowThresholdsMet {
                    distinct_inputs: 20,
                    validation_rate: 1.0,
                    approved: true,
                },
                actor: ArtifactActor::System,
                evidence_snapshot_hash: Some("a".repeat(64)),
            },
        )
        .unwrap();
    artifacts
        .transition(
            "tenant-a",
            &artifact.id,
            1,
            ArtifactTransition {
                expected: ArtifactStatus::Canary,
                trigger: ArtifactTrigger::CanaryThresholdsMet {
                    distinct_inputs: 20,
                    downstream_success: 1.0,
                    guard_violations: 0,
                },
                actor: ArtifactActor::System,
                evidence_snapshot_hash: Some("b".repeat(64)),
            },
        )
        .unwrap();

    let cascade = runtime
        .steward_dependency_departure("tenant-a", "tool.departing@1")
        .unwrap();
    assert_eq!(cascade.demoted, vec!["flow.dependent@1"]);
    assert_eq!(cascade.capability_requests.len(), 1);
    assert_eq!(
        runtime
            .artifact_repository()
            .unwrap()
            .get("tenant-a", "flow.dependent", 1)
            .unwrap()
            .unwrap()
            .status,
        ArtifactStatus::Shadow
    );
    let demands = CapabilityRequestRepository::open(
        aelio_store::EmbeddedStore::open(directory.path()).unwrap(),
    )
    .unwrap()
    .list_open("tenant-a", 10)
    .unwrap();
    assert_eq!(demands.len(), 1);

    let proposal = runtime
        .steward_propose_promotion(
            "tenant-a",
            "flow.dependent@1",
            &serde_json::json!({"distinct_inputs":20,"success":1.0}),
        )
        .unwrap();
    assert_eq!(proposal.status, PromotionProposalStatus::Pending);
    assert_eq!(
        runtime
            .artifact_repository()
            .unwrap()
            .get("tenant-a", "flow.dependent", 1)
            .unwrap()
            .unwrap()
            .status,
        ArtifactStatus::Shadow,
        "a steward proposal must not directly promote"
    );
}
