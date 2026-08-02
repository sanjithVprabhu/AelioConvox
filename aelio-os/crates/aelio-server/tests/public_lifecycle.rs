use aelio_runtime::{Runtime, RuntimeConfig, DEFAULT_QUEUE_DEPTH};
use aelio_server::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "aelio-public-lifecycle-token";

async fn call(
    app: &Router,
    token: Option<&str>,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = if let Some(body) = body {
        request = request.header("content-type", "application/json");
        request
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    } else {
        request.body(Body::empty()).unwrap()
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

fn cases() -> Vec<Value> {
    (0..20)
        .map(|index| {
            json!({
                "input":{"turn":{"index":index}},
                "wakes":[],
                "expect_park":false,
                "expected":{"ok":true},
                "fixtures":[]
            })
        })
        .collect()
}

fn pure_flow(id: &str) -> Value {
    json!({
        "tenant":"tenant-a",
        "flow_id":id,
        "flow_rev":"1",
        "program":{"nid":"root","op":"Const","v":{"ok":true}},
        "targets":[],
        "prompts":[]
    })
}

fn reviewed_flow() -> Value {
    json!({
        "tenant":"tenant-a",
        "flow_id":"flow.reviewed",
        "flow_rev":"1",
        "program":{
            "nid":"risk_reach","op":"Branch","pred":{"lit":false},
            "then":{
                "nid":"external","op":"Call","id":"external.audit@1",
                "args":{"turn":{"pull":"turn"}},"into":"result"
            },
            "else":{"nid":"safe","op":"Const","v":{"ok":true}}
        },
        "targets":[{
            "id":"external.audit@1","class":"tool","effect":"external",
            "input_imprint":"aelio.turn.input@1",
            "output_imprint":"aelio.turn.output@1",
            "bounded":{"kind":"deadline","max_ms":1000},
            "policy_tags":["reviewed-test"],"origin":"tenant"
        }],
        "prompts":[]
    })
}

fn runtime(root: &std::path::Path) -> Runtime {
    Runtime::open(RuntimeConfig {
        data_dir: root.into(),
        host_token: None,
        host_url: None,
        event_key_secret: [81; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap()
}

#[tokio::test]
async fn reviewed_artifact_is_inert_until_public_deployer_approval() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let insecure = aelio_server::router(ServerState::new(runtime.clone(), vec![], true).unwrap());
    let secure =
        aelio_server::router(ServerState::new(runtime.clone(), vec![TOKEN.into()], false).unwrap());
    let flow = reviewed_flow();

    let (status, pushed) = call(&insecure, None, "POST", "/v1/flows", Some(flow.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{pushed}");
    let (status, unapproved) = call(
        &insecure,
        None,
        "POST",
        "/v1/flows/gate",
        Some(json!({"flow":flow.clone(),"cases":cases()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unapproved}");
    assert_eq!(unapproved["record"]["status"], "shadow");

    let turn = json!({
        "tenant":"tenant-a","instance_id":"reviewed-before-approval",
        "flow_id":"flow.reviewed","flow_rev":"1","input":{"turn":{"index":99}}
    });
    let (status, denied) = call(&insecure, None, "POST", "/v1/turns", Some(turn)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{denied}");

    let (status, approved) = call(
        &secure,
        Some(TOKEN),
        "POST",
        "/v1/flows/gate",
        Some(json!({"flow":flow,"cases":cases()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["record"]["status"], "canary");
    assert!(approved["record"]["history"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["actor"]["deployer"]
        .as_str()
        .is_some_and(|actor| actor.starts_with("api-key-")));

    let (status, consumed) = call(
        &secure,
        Some(TOKEN),
        "POST",
        "/v1/turns",
        Some(json!({
            "tenant":"tenant-a","instance_id":"reviewed-after-approval",
            "flow_id":"flow.reviewed","flow_rev":"1","input":{"turn":{"index":99}}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{consumed}");
    assert_eq!(consumed["bag"]["ok"], true);
}

#[tokio::test]
async fn public_canary_evidence_promotes_and_guard_demotes_immediately() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path());
    let app = aelio_server::router(ServerState::new(runtime, vec![TOKEN.into()], false).unwrap());
    let flow = pure_flow("flow.lifecycle");
    let (status, pushed) = call(&app, Some(TOKEN), "POST", "/v1/flows", Some(flow.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{pushed}");
    let (status, gated) = call(
        &app,
        Some(TOKEN),
        "POST",
        "/v1/flows/gate",
        Some(json!({"flow":flow,"cases":cases()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{gated}");
    assert_eq!(gated["record"]["status"], "canary");

    let mut proposal_id = None;
    for index in 0..20 {
        let input_hash = blake3::hash(format!("input-{index}").as_bytes())
            .to_hex()
            .to_string();
        let ledger_hash = blake3::hash(format!("ledger-{index}").as_bytes())
            .to_hex()
            .to_string();
        let (status, observed) = call(
            &app,
            Some(TOKEN),
            "POST",
            "/v1/artifacts/canary-evidence",
            Some(json!({
                "tenant":"tenant-a","artifact_id":"flow.lifecycle","artifact_version":1,
                "observation":{
                    "input_hash":input_hash,"downstream_success":true,
                    "guard_violation":false,"ledger_hash":ledger_hash
                }
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{observed}");
        assert_eq!(observed["record"]["status"], "canary");
        if index == 19 {
            proposal_id = observed["promotion_proposal"]["proposal_id"]
                .as_str()
                .map(str::to_owned);
        } else {
            assert!(observed["promotion_proposal"].is_null());
        }
    }
    let proposal_id = proposal_id.expect("twenty distinct successful inputs must propose");
    let (status, duplicate_evidence) = call(
        &app,
        Some(TOKEN),
        "POST",
        "/v1/artifacts/canary-evidence",
        Some(json!({
            "tenant":"tenant-a","artifact_id":"flow.lifecycle","artifact_version":1,
            "observation":{
                "input_hash":blake3::hash(b"input-19").to_hex().to_string(),
                "downstream_success":true,"guard_violation":false,
                "ledger_hash":blake3::hash(b"ledger-19").to_hex().to_string()
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{duplicate_evidence}");
    assert_eq!(duplicate_evidence["evidence"]["distinct_inputs"], 20);
    assert_eq!(
        duplicate_evidence["promotion_proposal"]["proposal_id"],
        proposal_id
    );
    let (status, promoted) = call(
        &app,
        Some(TOKEN),
        "POST",
        &format!("/v1/artifacts/promotion-proposals/{proposal_id}/apply"),
        Some(json!({"tenant":"tenant-a"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{promoted}");
    assert_eq!(promoted["record"]["status"], "promoted");
    assert_eq!(promoted["promotion_proposal"]["status"], "applied");
    let (status, reapplied) = call(
        &app,
        Some(TOKEN),
        "POST",
        &format!("/v1/artifacts/promotion-proposals/{proposal_id}/apply"),
        Some(json!({"tenant":"tenant-a"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reapplied}");
    assert_eq!(reapplied["record"]["status"], "promoted");
    assert_eq!(reapplied["promotion_proposal"]["status"], "applied");

    let (status, consumed) = call(
        &app,
        Some(TOKEN),
        "POST",
        "/v1/turns",
        Some(json!({
            "tenant":"tenant-a","instance_id":"promoted-consumption",
            "flow_id":"flow.lifecycle","flow_rev":"1","input":{"turn":{"index":100}}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{consumed}");

    let (status, demoted) = call(
        &app,
        Some(TOKEN),
        "POST",
        "/v1/artifacts/canary-evidence",
        Some(json!({
            "tenant":"tenant-a","artifact_id":"flow.lifecycle","artifact_version":1,
            "observation":{
                "input_hash":blake3::hash(b"unsafe-input").to_hex().to_string(),
                "downstream_success":true,"guard_violation":true,
                "ledger_hash":blake3::hash(b"guard-ledger").to_hex().to_string()
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{demoted}");
    assert_eq!(demoted["record"]["status"], "shadow");
    assert_eq!(demoted["evidence"]["guard_violations"], 1);
    let demotion = demoted["record"]["history"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(demotion["from"], "promoted");
    assert_eq!(demotion["to"], "shadow");
    assert_eq!(demotion["actor"], "system");
    assert_eq!(demotion["trigger"]["kind"], "attributed_failure");

    let (status, denied) = call(
        &app,
        Some(TOKEN),
        "POST",
        "/v1/turns",
        Some(json!({
            "tenant":"tenant-a","instance_id":"after-guard-demotion",
            "flow_id":"flow.lifecycle","flow_rev":"1","input":{"turn":{"index":101}}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{denied}");
}
