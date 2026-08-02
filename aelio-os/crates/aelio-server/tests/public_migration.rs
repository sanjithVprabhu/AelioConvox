use aelio_runtime::{
    Artifact, ArtifactActor, ArtifactClass, ArtifactEffect, ArtifactInput, ArtifactInterface,
    ArtifactPins, ArtifactStatus, ArtifactTier, ArtifactTransition, ArtifactTrigger, Provenance,
    Runtime, RuntimeConfig, DEFAULT_QUEUE_DEPTH,
};
use aelio_server::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "aelio-public-migration-token";

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn artifact(id: &str, pins: ArtifactPins, output: &str, kernel: &str, learned: bool) -> Artifact {
    Artifact::new(
        id,
        1,
        ArtifactClass::Flow,
        ArtifactTier::Auto,
        "1",
        kernel,
        ArtifactInterface {
            inputs: vec![ArtifactInput {
                name: "turn".into(),
                imprint: "aelio.turn.input@1".into(),
                required: true,
                sensitivity: "internal".into(),
            }],
            output: output.into(),
        },
        format!("migration fixture {id}"),
        vec!["migration-fixture".into()],
        vec![ArtifactEffect::Pure],
        pins,
        vec![],
        json!({"program":{"nid":"root","op":"Const","v":{"ok":true}}}),
        Provenance {
            built_by: learned.then(|| "aelio.builder@1".into()),
            ..Provenance::default()
        },
    )
    .unwrap()
}

fn promote(runtime: &Runtime, artifact: Artifact) {
    let mut repository = runtime.artifact_repository().unwrap();
    repository
        .put_proposed("tenant-a", artifact.clone(), ArtifactActor::System)
        .unwrap();
    repository
        .transition(
            "tenant-a",
            &artifact.id,
            artifact.version,
            ArtifactTransition {
                expected: ArtifactStatus::Proposed,
                trigger: ArtifactTrigger::StructuralPass,
                actor: ArtifactActor::System,
                evidence_snapshot_hash: Some(artifact.hash.clone()),
            },
        )
        .unwrap();
    repository
        .transition(
            "tenant-a",
            &artifact.id,
            artifact.version,
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
    repository
        .transition(
            "tenant-a",
            &artifact.id,
            artifact.version,
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
}

#[tokio::test]
async fn public_dependency_and_kernel_migrations_apply_exact_conservative_cascades() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(RuntimeConfig {
        data_dir: root.path().into(),
        host_token: None,
        host_url: None,
        event_key_secret: [91; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();

    promote(
        &runtime,
        artifact(
            "flow.prompt-dependent",
            ArtifactPins {
                prompts: vec!["prompt.answer@1".into()],
                ..ArtifactPins::default()
            },
            "aelio.turn.output@1",
            env!("CARGO_PKG_VERSION"),
            true,
        ),
    );
    promote(
        &runtime,
        artifact(
            "flow.model-dependent",
            ArtifactPins {
                models: vec!["model.answer@1".into()],
                ..ArtifactPins::default()
            },
            "aelio.turn.output@1",
            env!("CARGO_PKG_VERSION"),
            true,
        ),
    );
    promote(
        &runtime,
        artifact(
            "flow.embedding-dependent",
            ArtifactPins {
                embeddings: vec!["embedding.answer@1".into()],
                ..ArtifactPins::default()
            },
            "aelio.turn.output@1",
            env!("CARGO_PKG_VERSION"),
            true,
        ),
    );
    promote(
        &runtime,
        artifact(
            "flow.imprint-dependent",
            ArtifactPins::default(),
            "schema.customer@1",
            env!("CARGO_PKG_VERSION"),
            true,
        ),
    );
    promote(
        &runtime,
        artifact(
            "flow.old-kernel-learned",
            ArtifactPins::default(),
            "aelio.turn.output@1",
            "0.0.0-old",
            true,
        ),
    );
    promote(
        &runtime,
        artifact(
            "flow.old-kernel-human",
            ArtifactPins::default(),
            "aelio.turn.output@1",
            "0.0.0-old",
            false,
        ),
    );

    let app =
        aelio_server::router(ServerState::new(runtime.clone(), vec![TOKEN.into()], false).unwrap());
    for (dependency, expected) in [
        ("prompt.answer@1", "flow.prompt-dependent@1"),
        ("model.answer@1", "flow.model-dependent@1"),
        ("embedding.answer@1", "flow.embedding-dependent@1"),
        ("schema.customer@1", "flow.imprint-dependent@1"),
    ] {
        let (status, cascade) = post(
            &app,
            "/v1/system/dependencies/departure",
            json!({"tenant":"tenant-a","dependency_pin":dependency}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{cascade}");
        assert_eq!(cascade["dependency"], dependency);
        assert_eq!(cascade["demoted"], json!([expected]));
        assert_eq!(cascade["capability_requests"].as_array().unwrap().len(), 1);
    }

    let (status, migration) = post(
        &app,
        "/v1/system/kernel-migration",
        json!({"tenant":"tenant-a"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{migration}");
    assert_eq!(migration["kernel_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(migration["demoted"], json!(["flow.old-kernel-learned@1"]));
    let repository = runtime.artifact_repository().unwrap();
    assert_eq!(
        repository
            .get("tenant-a", "flow.old-kernel-learned", 1)
            .unwrap()
            .unwrap()
            .status,
        ArtifactStatus::Canary
    );
    assert_eq!(
        repository
            .get("tenant-a", "flow.old-kernel-human", 1)
            .unwrap()
            .unwrap()
            .status,
        ArtifactStatus::Promoted
    );
    let (status, refused_old_kernel) = post(
        &app,
        "/v1/turns",
        json!({
            "tenant":"tenant-a","instance_id":"old-kernel-refusal",
            "flow_id":"flow.old-kernel-human","flow_rev":"1","input":{"turn":{}}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused_old_kernel}");
    assert!(refused_old_kernel["error"]["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("explicit migration is required")));

    let (status, replay) = post(
        &app,
        "/v1/system/kernel-migration",
        json!({"tenant":"tenant-a"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["demoted"], json!([]));
}
