use aelio_runtime::{Runtime, RuntimeConfig, DEFAULT_QUEUE_DEPTH};
use aelio_server::ServerState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "aelio-public-build-token";

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"));
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
                "input":{"turn":index},
                "wakes":[],
                "expect_park":false,
                "expected":{"ok":true},
                "fixtures":[]
            })
        })
        .collect()
}

fn build_spec() -> Value {
    json!({
        "name":"flow.built",
        "description":"immutable flow flow seed",
        "inputs":[{
            "name":"turn","imprint":"aelio.turn.input@1","required":true,"sensitivity":"internal"
        },{
            "name":"context","imprint":"aelio.turn.input@1","required":false,"sensitivity":"internal"
        }],
        "output":"aelio.turn.output@1",
        "budget":{
            "max_depth":4,"max_children":4,"max_llm_calls":4,"max_tokens":10000,
            "max_reactions":100,"max_wall_ms":60000
        },
        "scope":{"tenant":"tenant-a","registries":["flow.*"]},
        "policy":{
            "principal_grants":[],"allowed_effects":["pure"],
            "denied_effects":["write","external"]
        },
        "examples":(0..20).map(|index| json!({
            "inputs":{"turn":{"turn":index}},
            "output":{"ok":true},
            "negative":index == 0,
            "fixtures":[]
        })).collect::<Vec<_>>()
    })
}

#[tokio::test]
async fn public_demand_builds_and_sandbox_gates_a_later_canary() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(RuntimeConfig {
        data_dir: root.path().into(),
        host_token: None,
        host_url: None,
        event_key_secret: [73; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();
    let app =
        aelio_server::router(ServerState::new(runtime.clone(), vec![TOKEN.into()], false).unwrap());

    let demand_body = json!({
        "tenant":"tenant-a",
        "draft":{
            "normalized_need":"immutable flow flow seed",
            "inputs":[{
                "name":"turn","imprint":"aelio.turn.input@1","required":true,"sensitivity":"internal"
            },{
                "name":"context","imprint":"aelio.turn.input@1","required":false,"sensitivity":"internal"
            }],
            "output":"aelio.turn.output@1",
            "allowed_effects":["pure"],
            "requester":"body-cannot-choose-principal",
            "reason":"missing_capability",
            "evidence_refs":["turn:missing-capability"]
        }
    });
    let (status, first_demand) = call(
        &app,
        "POST",
        "/v1/capability-requests",
        Some(demand_body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first_demand}");
    let request_id = first_demand["request_id"].as_str().unwrap().to_owned();
    assert_eq!(first_demand["status"], "open");
    assert!(first_demand["requester"]
        .as_str()
        .unwrap()
        .starts_with("api-key-"));

    let (status, duplicate_demand) =
        call(&app, "POST", "/v1/capability-requests", Some(demand_body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{duplicate_demand}");
    assert_eq!(duplicate_demand["request_id"], request_id);
    assert_eq!(duplicate_demand["demand_count"], 2);

    let seed = json!({
        "tenant":"tenant-a",
        "flow_id":"flow.seed",
        "flow_rev":"1",
        "program":{"nid":"root","op":"Const","v":{"ok":true}},
        "targets":[],
        "prompts":[]
    });
    let (status, pushed) = call(&app, "POST", "/v1/flows", Some(seed.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{pushed}");
    let (status, admitted_seed) = call(
        &app,
        "POST",
        "/v1/flows/gate",
        Some(json!({"flow":seed,"cases":cases()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{admitted_seed}");
    assert_eq!(admitted_seed["record"]["status"], "canary");

    let (status, queued) = call(
        &app,
        "POST",
        &format!("/v1/capability-requests/{request_id}/queue"),
        Some(json!({"tenant":"tenant-a","spec":build_spec()})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    let build_id = queued["job"]["build_id"].as_str().unwrap().to_owned();
    assert_eq!(queued["request"]["status"], "queued");

    let mut completed = None;
    for _ in 0..16 {
        let (status, action) = call(
            &app,
            "POST",
            &format!("/v1/builds/{build_id}/advance"),
            Some(json!({"tenant":"tenant-a"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{action}");
        match action["action"].as_str().unwrap() {
            "progress" => {}
            "oracle" => {
                let reaction_id = action["reaction"]["reaction_id"].as_str().unwrap();
                let output = match action["reaction"]["kind"].as_str().unwrap() {
                    "select" => json!({
                        "kind":"selection",
                        "selected":[{"id":"flow.seed@1","score":0.99,"role":"seed"}],
                        "runners_up":[],"unmet":[],"undeterminable":false
                    }),
                    "compose" => json!({
                        "kind":"composition",
                        "tree":[{"nid":"seed","artifact":"flow.seed@1"}],
                        "seams":[
                            {"from":{"kind":"parent_input","slot":"turn"},
                             "to":{"kind":"node_input","node":"seed","slot":"turn"}},
                            {"from":{"kind":"node_output","node":"seed"},
                             "to":{"kind":"parent_output"}}
                        ],
                        "undeterminable":false
                    }),
                    other => panic!("unexpected oracle kind {other}: {action}"),
                };
                let (reaction_status, reacted) = call(
                    &app,
                    "POST",
                    &format!("/v1/builds/{build_id}/reaction"),
                    Some(json!({
                        "tenant":"tenant-a",
                        "reaction_id":reaction_id,
                        "output":output,
                        "usage":{"model_calls":1,"tokens":10,"wall_ms":1}
                    })),
                )
                .await;
                assert_eq!(reaction_status, StatusCode::OK, "{reacted}");
            }
            "complete" => {
                completed = Some(action);
                break;
            }
            other => panic!("unknown build action {other}"),
        }
    }
    let completed = completed.expect("bounded scripted build must reach a terminal action");
    assert_eq!(completed["result"]["status"], "built", "{completed}");
    assert_eq!(completed["result"]["artifact"]["id"], "flow.built");
    assert_eq!(completed["result"]["artifact"]["version"], 1);
    assert!(completed["result"]["gate_verdict_hash"]
        .as_str()
        .is_some_and(|hash| hash.len() == 64));

    let (status, demand_after) = call(
        &app,
        "GET",
        &format!("/v1/capability-requests/{request_id}?tenant=tenant-a"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{demand_after}");
    assert_eq!(demand_after["status"], "resolved");
    assert_eq!(demand_after["build_id"], build_id);

    let turn = json!({
        "tenant":"tenant-a",
        "instance_id":"public-built-harness-1",
        "flow_id":"flow.built",
        "flow_rev":"1",
        "input":{"turn":{"turn":99}}
    });
    let (status, first) = call(&app, "POST", "/v1/turns", Some(turn.clone())).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["outcome"], "completed", "{first}");
    assert_eq!(first["bag"]["ok"], true);
    let (status, replay) = call(&app, "POST", "/v1/turns", Some(turn)).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay, first);
}
