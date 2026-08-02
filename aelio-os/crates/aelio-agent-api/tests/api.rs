use aelio_agent::runtime::{DurableRuntime, World};
use aelio_agent::storage::AelioStore;
use aelio_agent_api::{router, AppState};
use aelio_db_query::Database;
use aelio_runtime::{
    ArtifactStatus, BoundSpec, EffectSpec, FlowPush, OriginSpec, Runtime, RuntimeConfig,
    SandboxCase, SandboxFixtureCall, SandboxLimits, TargetClassSpec, TargetSpec,
    DEFAULT_QUEUE_DEPTH,
};
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tower::ServiceExt;

fn app(tag: &str, keys: Vec<String>) -> axum::Router {
    let path = std::env::temp_dir().join(format!("aelio_server_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let store = AelioStore::new(Database::create(path).unwrap(), 3).unwrap();
    let runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
    router(AppState::new(runtime, keys))
}

fn empty_app(tag: &str, keys: Vec<String>) -> axum::Router {
    let path =
        std::env::temp_dir().join(format!("aelio_server_empty_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let store = AelioStore::new(Database::create(path).unwrap(), 3).unwrap();
    let runtime = DurableRuntime::new(World::empty_tenant("tenant-1").unwrap(), store).unwrap();
    router(AppState::new(runtime, keys))
}

fn scoped_app(tag: &str) -> axum::Router {
    let path =
        std::env::temp_dir().join(format!("aelio_server_scoped_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let store = AelioStore::new(Database::create(path).unwrap(), 3).unwrap();
    let runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
    router(AppState::new_with_scoped_sdk_bridge(
        runtime,
        vec!["runtime-key".into()],
        vec!["admin-key".into()],
        Default::default(),
    ))
}

fn unified_app(tag: &str) -> (axum::Router, Runtime) {
    unified_app_with_host(tag, None)
}

fn unified_app_with_host(tag: &str, host_url: Option<String>) -> (axum::Router, Runtime) {
    let root =
        std::env::temp_dir().join(format!("aelio_server_unified_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let agent_path = root.join("agent");
    std::fs::create_dir_all(&agent_path).unwrap();
    let store = AelioStore::new(Database::create(agent_path).unwrap(), 3).unwrap();
    let agent = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
    let artifact_runtime = Runtime::open(RuntimeConfig {
        data_dir: root.join("runtime"),
        host_token: host_url.as_ref().map(|_| "test-host-token".into()),
        host_url,
        event_key_secret: [23; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();
    (
        router(AppState::new_with_artifact_runtime(
            agent,
            vec![],
            artifact_runtime.clone(),
        )),
        artifact_runtime,
    )
}

#[derive(Clone)]
struct FakeHostState {
    calls: std::sync::Arc<tokio::sync::Mutex<Vec<Value>>>,
}

async fn fake_host_target(
    State(state): State<FakeHostState>,
    Json(request): Json<Value>,
) -> Json<Value> {
    state.calls.lock().await.push(request.clone());
    let output = match request["target"].as_str() {
        Some("send_otp@1") => json!({"ok":true,"continuation":"auth.otp.verify"}),
        Some("verify_otp@1") => json!({"ok":true}),
        _ => json!({"ok":true}),
    };
    Json(json!({
        "outcome":"ok",
        "output":output,
        "error":null,
        "usage_tokens":0
    }))
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    let request = if let Some(body) = body {
        request
            .header("content-type", "application/json")
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

#[tokio::test]
async fn public_sdk_socket_acks_and_discards_duplicate_tool_results() {
    let root = std::env::temp_dir().join(format!(
        "aelio_server_sdk_duplicate_result_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let store = AelioStore::new(Database::create(root).unwrap(), 3).unwrap();
    let runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
    let state = AppState::new_with_scoped_sdk_bridge(
        runtime,
        vec!["runtime-key".into()],
        vec!["admin-key".into()],
        aelio_agent_api::sdk_bridge::BridgeConfig {
            invocation_timeout: std::time::Duration::from_secs(2),
            max_in_flight_per_tenant: 4,
            outbound_capacity: 8,
        },
    );
    let app = router(state);
    let direct_app = app.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let mut request = format!("ws://{address}/v1/sdk")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer runtime-key".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket
        .send(WsMessage::Text(
            json!({
                "type":"register",
                "catalog":World::demo_tenant("tenant-1").tenant,
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let registration: Value = serde_json::from_str(
        socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_str(),
    )
    .unwrap();
    assert_eq!(registration["type"], "register_ack");
    assert_eq!(registration["status"], "active");

    let turn_request = json!({
        "turn_id":"sdk-duplicate-turn",
        "user_id":"sdk-duplicate-user",
        "utterance":"login with +15551234567",
        "channel":"web"
    });
    let invoking = {
        let app = direct_app.clone();
        let request = turn_request.clone();
        tokio::spawn(async move {
            call(
                &app,
                "POST",
                "/v1/turns",
                Some("runtime-key"),
                Some(request),
            )
            .await
        })
    };
    let invocation: Value = serde_json::from_str(
        socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_str(),
    )
    .unwrap();
    assert_eq!(invocation["type"], "invoke");
    assert_eq!(invocation["tool_id"], "send_otp");
    assert_eq!(invocation["tool_version"], "1");
    let invocation_id = invocation["invocation_id"].as_str().unwrap().to_owned();

    socket
        .send(WsMessage::Text(
            json!({
                "type":"result",
                "invocation_id":invocation_id,
                "ok":true,
                "data":true,
                "error":null,
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let accepted: Value = serde_json::from_str(
        socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_str(),
    )
    .unwrap();
    assert_eq!(accepted["type"], "result_ack");
    assert_eq!(accepted["invocation_id"], invocation_id);
    assert_eq!(accepted["disposition"], "accepted");
    let (status, first_turn) = invoking.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{first_turn}");
    assert_eq!(first_turn["suspended"], true);
    assert!(first_turn["steps"].as_array().unwrap().iter().any(|step| {
        step["name"] == "Invoke.Call"
            && step["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("send_otp"))
    }));
    let delivery_trace = first_turn["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Artifact.Ledger")
        .expect("public turn must render the metadata-only aelio-wire delivery ledger");
    let delivery_trace: Value =
        serde_json::from_str(delivery_trace["detail"].as_str().unwrap()).unwrap();
    assert_eq!(delivery_trace["authority"], "aelio-wire");
    assert_eq!(
        delivery_trace["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| step["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["call_intent", "call_dispatch", "call_result"]
    );

    socket
        .send(WsMessage::Text(
            json!({
                "type":"result",
                "invocation_id":invocation_id,
                "ok":true,
                "data":false,
                "error":null,
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let duplicate: Value = serde_json::from_str(
        socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_str(),
    )
    .unwrap();
    assert_eq!(duplicate["type"], "result_ack");
    assert_eq!(duplicate["invocation_id"], invocation_id);
    assert_eq!(duplicate["disposition"], "duplicate");

    let (status, delivery_audit) = call(
        &direct_app,
        "GET",
        "/v1/admin/sdk-delivery?limit=10",
        Some("admin-key"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{delivery_audit}");
    let delivery_events = delivery_audit["items"].as_array().unwrap();
    assert_eq!(delivery_events.len(), 2, "{delivery_audit}");
    let dispositions = delivery_events
        .iter()
        .map(|item| item["event"]["disposition"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        dispositions,
        std::collections::BTreeSet::from(["accepted", "duplicate"])
    );
    let correlation_hashes = delivery_events
        .iter()
        .map(|item| item["event"]["correlation_hash"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(correlation_hashes.len(), 1);
    assert_eq!(correlation_hashes.first().unwrap().len(), 64);
    assert!(
        !delivery_audit.to_string().contains(&invocation_id),
        "the durable audit boundary must never expose the raw SDK correlation"
    );

    let (status, replay) = call(
        &direct_app,
        "POST",
        "/v1/turns",
        Some("runtime-key"),
        Some(turn_request),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["reply"], first_turn["reply"]);
    assert_eq!(replay["steps"], first_turn["steps"]);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), socket.next())
            .await
            .is_err(),
        "duplicate turn emitted another tool invocation"
    );

    socket.close(None).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn public_sdk_disconnect_distinguishes_pre_dispatch_from_unknown_outcome() {
    let root = std::env::temp_dir().join(format!(
        "aelio_server_sdk_disconnect_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let store = AelioStore::new(Database::create(root).unwrap(), 3).unwrap();
    let runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
    let state = AppState::new_with_scoped_sdk_bridge(
        runtime,
        vec!["runtime-key".into()],
        vec!["admin-key".into()],
        aelio_agent_api::sdk_bridge::BridgeConfig {
            invocation_timeout: std::time::Duration::from_millis(100),
            max_in_flight_per_tenant: 4,
            outbound_capacity: 8,
        },
    );
    let bridge = state.bridge();
    let app = router(state);
    let direct_app = app.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    async fn connect_and_register(
        address: std::net::SocketAddr,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = format!("ws://{address}/v1/sdk")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", "Bearer runtime-key".parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        socket
            .send(WsMessage::Text(
                json!({
                    "type":"register",
                    "catalog":World::demo_tenant("tenant-1").tenant,
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let ack: Value = serde_json::from_str(
            socket
                .next()
                .await
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap()
                .as_str(),
        )
        .unwrap();
        assert_eq!(ack["type"], "register_ack");
        socket
    }

    // Disconnecting while idle is provably pre-dispatch. The public admission gate fails before
    // creating an effect intent or an idempotency record.
    let mut idle_socket = connect_and_register(address).await;
    idle_socket.close(None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if bridge
                .catalog_status("tenant-1")
                .is_some_and(|status| !status.available)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (status, offline) = call(
        &direct_app,
        "POST",
        "/v1/turns",
        Some("runtime-key"),
        Some(json!({
            "turn_id":"sdk-offline-before-dispatch",
            "user_id":"sdk-disconnect-user",
            "utterance":"login with +15551234567",
            "channel":"web"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{offline}");
    assert_eq!(offline["code"], "unavailable");
    let (status, outcomes) = call(
        &direct_app,
        "GET",
        "/v1/admin/tool-outcomes?limit=10",
        Some("admin-key"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{outcomes}");
    assert!(outcomes["items"].as_array().unwrap().is_empty());

    // Once the invocation frame crossed the socket write boundary, loss of the socket is an
    // unknown external outcome. It must not be downgraded to retryable unavailability.
    let mut dispatched_socket = connect_and_register(address).await;
    let invoking = {
        let app = direct_app.clone();
        tokio::spawn(async move {
            call(
                &app,
                "POST",
                "/v1/turns",
                Some("runtime-key"),
                Some(json!({
                    "turn_id":"sdk-disconnect-after-dispatch",
                    "user_id":"sdk-disconnect-user",
                    "utterance":"login with +15551234567",
                    "channel":"web"
                })),
            )
            .await
        })
    };
    let invocation: Value = serde_json::from_str(
        dispatched_socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_str(),
    )
    .unwrap();
    assert_eq!(invocation["type"], "invoke");
    assert_eq!(invocation["tool_id"], "send_otp");
    assert_eq!(invocation["tool_version"], "1");
    let invocation_id = invocation["invocation_id"].as_str().unwrap().to_owned();
    dispatched_socket.close(None).await.unwrap();

    let (status, unknown) = invoking.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{unknown}");
    assert_eq!(unknown["suspended"], true);
    assert!(unknown["steps"].as_array().unwrap().iter().any(|step| {
        step["name"] == "Invoke.Error"
            && step["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("reason=NeedsEscalation"))
    }));
    let ledger = unknown["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Artifact.Ledger")
        .expect("post-dispatch loss must retain its authoritative delivery ledger");
    let ledger: Value = serde_json::from_str(ledger["detail"].as_str().unwrap()).unwrap();
    assert_eq!(ledger["authority"], "aelio-wire");
    assert_eq!(
        ledger["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| step["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["call_intent", "call_dispatch"]
    );

    let (status, outcomes) = call(
        &direct_app,
        "GET",
        "/v1/admin/tool-outcomes?limit=10",
        Some("admin-key"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{outcomes}");
    let outcomes = outcomes["items"].as_array().unwrap();
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0]["tool_id"], "send_otp");
    assert_eq!(outcomes[0]["tool_version"], "1");
    assert_eq!(outcomes[0]["state"], "manual_review");
    assert_eq!(outcomes[0]["reason_code"], "internal");
    assert_eq!(outcomes[0]["idempotency_hash"].as_str().unwrap().len(), 64);
    assert_eq!(outcomes[0]["subject_hash"].as_str().unwrap().len(), 64);
    assert!(!serde_json::to_string(outcomes)
        .unwrap()
        .contains("sdk-disconnect-user"));

    // A later result is informational only. It receives an explicit late acknowledgement and a
    // late-result ledger entry, but it cannot resume the turn or rewrite manual-review state.
    let mut late_socket = connect_and_register(address).await;
    late_socket
        .send(WsMessage::Text(
            json!({
                "type":"result",
                "invocation_id":invocation_id,
                "ok":true,
                "data":true,
                "error":null,
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let late: Value = serde_json::from_str(
        late_socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap()
            .as_str(),
    )
    .unwrap();
    assert_eq!(late["type"], "result_ack");
    assert_eq!(late["disposition"], "late");
    assert_eq!(
        late["delivery_ledger"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| step["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["call_intent", "call_dispatch", "late_result"]
    );
    let (status, delivery_events) = call(
        &direct_app,
        "GET",
        "/v1/admin/sdk-delivery?limit=10",
        Some("admin-key"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{delivery_events}");
    assert_eq!(delivery_events["items"].as_array().unwrap().len(), 1);
    assert_eq!(delivery_events["items"][0]["event"]["disposition"], "late");
    let (status, outcomes_after_late) = call(
        &direct_app,
        "GET",
        "/v1/admin/tool-outcomes?limit=10",
        Some("admin-key"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{outcomes_after_late}");
    assert_eq!(outcomes_after_late["items"].as_array().unwrap().len(), 1);
    assert_eq!(outcomes_after_late["items"][0]["state"], "manual_review");

    server.abort();
}

#[tokio::test]
async fn admitted_bound_procedure_executes_through_unified_runtime_and_replays() {
    let root =
        std::env::temp_dir().join(format!("aelio_server_adaptive_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let world = World::demo_tenant("tenant-1");

    let agent_path = root.join("agent");
    std::fs::create_dir_all(&agent_path).unwrap();
    let store = AelioStore::new(Database::create(agent_path).unwrap(), 3).unwrap();
    let agent = DurableRuntime::new(world, store).unwrap();
    let artifact_runtime = Runtime::open(RuntimeConfig {
        data_dir: root.join("runtime"),
        host_token: None,
        host_url: None,
        event_key_secret: [35; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();
    let flow = FlowPush {
        tenant: "tenant-1".into(),
        flow_id: "procedure.greeting".into(),
        flow_rev: "1".into(),
        program: json!({
            "nid":"unified_reply","op":"Const","v":{"text":"Hello from Aelio OS"}
        }),
        targets: vec![],
        prompts: vec![],
    };
    artifact_runtime.push_flow(flow.clone()).unwrap();
    let cases = (0..20)
        .map(|index| SandboxCase {
            input: json!({"turn":{"case":index}}),
            wakes: vec![],
            expect_park: false,
            expected: json!({"text":"Hello from Aelio OS"}),
            fixtures: vec![],
        })
        .collect::<Vec<_>>();
    artifact_runtime
        .gate_flow(
            &flow,
            &cases,
            SandboxLimits::default(),
            Some("deployer:test".into()),
        )
        .unwrap();
    let record = artifact_runtime
        .artifact_repository()
        .unwrap()
        .get("tenant-1", "procedure.greeting", 1)
        .unwrap()
        .unwrap();
    let artifact = json!({
        "id":record.artifact.id,
        "version":record.artifact.version,
        "hash":record.artifact.hash,
    });
    let app = router(AppState::new_with_artifact_runtime(
        agent,
        vec!["admin-secret".into()],
        artifact_runtime,
    ));

    let mut cold_reply = None;
    for index in 0..5 {
        let (status, cold) = call(
            &app,
            "POST",
            "/v1/turns",
            Some("admin-secret"),
            Some(json!({
                "turn_id":format!("adaptive-cold-{index}"),
                "user_id":format!("adaptive-cold-user-{index}"),
                "utterance":"hi",
                "channel":"web"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{cold}");
        assert_eq!(cold["tier"], "tier2");
        assert_eq!(cold["llm_calls"], 0);
        assert_eq!(cold["new_state"], Value::Null);
        assert_eq!(cold["active_flow"], Value::Null);
        assert_eq!(cold["suspended"], false);
        assert!(!cold["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step["name"] == "Artifact.Ledger"));
        if let Some(expected) = &cold_reply {
            assert_eq!(&cold["reply"]["text"], expected);
        } else {
            cold_reply = Some(cold["reply"]["text"].clone());
        }
    }
    let (status, procedures) = call(
        &app,
        "GET",
        "/v1/admin/procedures?limit=10",
        Some("admin-secret"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{procedures}");
    let promoted = procedures["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["procedure"]["spec"]["situation_filter"]["intent_class"] == "greeting")
        .expect("five public cold successes must produce the reviewed greeting procedure");
    let procedure_id = promoted["procedure"]["procedure_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let (status, binding) = call(
        &app,
        "POST",
        &format!("/v1/admin/procedures/{procedure_id}/bind-artifact"),
        Some("admin-secret"),
        Some(artifact),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{binding}");
    assert_eq!(binding["binding"], "immutable");

    let request = json!({
        "turn_id":"adaptive-warm-1",
        "user_id":"adaptive-warm-user",
        "utterance":"hi",
        "channel":"web"
    });
    let (status, first) = call(
        &app,
        "POST",
        "/v1/turns",
        Some("admin-secret"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["tier"], "tier0");
    assert_eq!(first["llm_calls"], 0);
    assert_eq!(first["new_state"], Value::Null);
    assert_eq!(first["active_flow"], Value::Null);
    assert_eq!(first["suspended"], false);
    assert_eq!(first["reply"]["text"], "Hello from Aelio OS");
    assert!(first["steps"].as_array().unwrap().iter().any(|step| {
        step["name"] == "AdaptiveInvoke"
            && step["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("authority=aelio-runtime"))
    }));
    let ledger = first["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Artifact.Ledger")
        .expect("warm invocation must render the authoritative runtime ledger");
    let ledger: Value = serde_json::from_str(ledger["detail"].as_str().unwrap()).unwrap();
    assert_eq!(ledger["artifact"], "procedure.greeting@1");
    assert!(!ledger["steps"].as_array().unwrap().iter().any(|step| {
        matches!(
            step["kind"].as_str(),
            Some("call_intent" | "call_dispatch" | "call_result")
        )
    }));

    let (status, replay) = call(
        &app,
        "POST",
        "/v1/turns",
        Some("admin-secret"),
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["reply"], first["reply"]);
}

#[tokio::test]
async fn catalog_lowering_resolves_exact_capability_and_activates_runtime_owned_flow() {
    let calls = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let host = Router::new()
        .route("/internal/aelio/target", post(fake_host_target))
        .with_state(FakeHostState {
            calls: calls.clone(),
        });
    let server = tokio::spawn(async move { axum::serve(listener, host).await.unwrap() });
    let host_url = format!("http://{address}");
    let reopen_host_url = host_url.clone();
    let reopen_root = std::env::temp_dir().join(format!(
        "aelio_server_unified_catalog_lowering_{}",
        std::process::id()
    ));
    let (app, artifact_runtime) = tokio::task::spawn_blocking(move || {
        unified_app_with_host("catalog_lowering", Some(host_url))
    })
    .await
    .unwrap();
    let mut catalog = World::demo_tenant("tenant-1").tenant;
    catalog.flows[0].lowering = Some(
        serde_json::from_value(json!({
            "format":1,
            "program":{"nid":"login","op":"Seq","steps":[
                {"nid":"send_once","op":"Once","body":{
                    "nid":"send","op":"Call","id":"$cap:send",
                    "args":{
                        "args":{"pull":"turn"},
                        "context":{"lit":{"source":"catalog.lowering"}}
                    },
                    "into":"sent"
                }},
                {"nid":"ask","op":"Const","v":{"text":"Enter the six-digit code"}},
                {"nid":"wait_first","op":"Park","until":{"kind":"event"},"into":"otp_first"},
                {"nid":"check_first","op":"Branch",
                 "pred":{"fn":"eq","args":[
                     {"pull":"otp_first.turn.utterance"},{"lit":"123456"}
                 ]},
                 "then":{"nid":"verify_first","op":"Seq","steps":[
                     {"nid":"verify_call_first","op":"Call","id":"$cap:verify",
                      "args":{
                          "args":{"pull":"otp_first.turn"},
                          "context":{"lit":{"source":"catalog.lowering"}}
                      },"into":"verified_first"},
                     {"nid":"done_first","op":"Const","v":{"text":"Authenticated by lowered runtime flow"}}
                 ]},
                 "else":{"nid":"repair","op":"Seq","steps":[
                     {"nid":"ask_again","op":"Const","v":{"text":"Enter the six-digit code"}},
                     {"nid":"wait_again","op":"Park","until":{"kind":"event"},"into":"otp_second"},
                     {"nid":"check_second","op":"Branch",
                      "pred":{"fn":"eq","args":[
                          {"pull":"otp_second.turn.utterance"},{"lit":"123456"}
                      ]},
                      "then":{"nid":"verify_second","op":"Seq","steps":[
                          {"nid":"verify_call_second","op":"Call","id":"$cap:verify",
                           "args":{
                               "args":{"pull":"otp_second.turn"},
                               "context":{"lit":{"source":"catalog.lowering"}}
                           },"into":"verified_second"},
                          {"nid":"done_second","op":"Const","v":{"text":"Authenticated by lowered runtime flow"}}
                      ]},
                      "else":{"nid":"failed","op":"Const","v":{"text":"OTP verification failed"}}}
                 ]}}
            ]},
            "bindings":[
                {"name":"send","step_id":"collect_phone","capability":"auth.otp.send","deadline_ms":30000},
                {"name":"verify","step_id":"await_otp","capability":"auth.otp.verify","deadline_ms":30000}
            ],
            "cases":(0..20).map(|index| json!({
                "input":{"turn":{"utterance":format!("+9198765432{index:02}")}},
                "wakes":[
                    {"turn":{"utterance":"000000"}},
                    {"turn":{"utterance":"123456"}}
                ],
                "expect_park":false,
                "expected":{"text":"Authenticated by lowered runtime flow"},
                "fixtures":[
                    {"binding":"send","output":{"ok":true,"continuation":"auth.otp.verify"},"usage_tokens":0},
                    {"binding":"verify","output":{"ok":true},"usage_tokens":0}
                ]
            })).collect::<Vec<_>>()
        }))
        .unwrap(),
    );
    let catalog_value = serde_json::to_value(catalog).unwrap();
    let (status, registered) = call(
        &app,
        "POST",
        "/v1/catalog",
        None,
        Some(catalog_value.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{registered}");
    assert_eq!(registered["materialization_pending"], 0);
    let (status, repeated) = call(
        &app,
        "POST",
        "/v1/catalog",
        None,
        Some(catalog_value.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{repeated}");
    assert_eq!(repeated["materialization_pending"], 0);

    let (status, active) = call(&app, "GET", "/v1/catalog", None, None).await;
    assert_eq!(status, StatusCode::OK, "{active}");
    let pin = &active["flow_artifacts"]["login"];
    assert_eq!(pin["id"], "aelio.catalog.login");
    let record = artifact_runtime
        .artifact_repository()
        .unwrap()
        .get("tenant-1", "aelio.catalog.login", 1)
        .unwrap()
        .unwrap();
    assert!(matches!(
        record.status,
        ArtifactStatus::Canary | ArtifactStatus::Promoted
    ));
    assert_eq!(record.artifact.hash, pin["hash"].as_str().unwrap());
    assert_eq!(record.artifact.body["targets"][0]["id"], "send_otp@1");
    assert_eq!(record.artifact.body["targets"][1]["id"], "verify_otp@1");
    assert!(!record.artifact.body.to_string().contains("$cap:"));

    let mut drifted = catalog_value;
    drifted["tools"][0]["version"] = json!("2");
    let (status, rejected) = call(&app, "POST", "/v1/catalog", None, Some(drifted)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{rejected}");
    let (status, still_active) = call(&app, "GET", "/v1/catalog", None, None).await;
    assert_eq!(status, StatusCode::OK, "{still_active}");
    assert_eq!(still_active["flow_artifacts"]["login"], *pin);
    assert_eq!(still_active["tools"][0]["version"], "1");

    let (status, parked) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"lowered-login-1",
            "user_id":"lowered-user",
            "utterance":"log in",
            "channel":"web"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{parked}");
    assert!(parked["suspended"].as_bool().unwrap());
    assert_eq!(calls.lock().await.len(), 1);

    let (status, repaired) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"lowered-login-2",
            "user_id":"lowered-user",
            "utterance":"000000",
            "channel":"web"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{repaired}");
    assert!(repaired["suspended"].as_bool().unwrap());
    assert_eq!(calls.lock().await.len(), 1, "repair must not resend");

    tokio::task::spawn_blocking(move || drop((app, artifact_runtime)))
        .await
        .unwrap();
    let (app, artifact_runtime) = tokio::task::spawn_blocking(move || {
        let agent_store =
            AelioStore::new(Database::open(reopen_root.join("agent")).unwrap(), 3).unwrap();
        let agent = DurableRuntime::new(World::demo_tenant("tenant-1"), agent_store).unwrap();
        let artifact_runtime = Runtime::open(RuntimeConfig {
            data_dir: reopen_root.join("runtime"),
            host_token: Some("test-host-token".into()),
            host_url: Some(reopen_host_url),
            event_key_secret: [23; 32],
            queue_depth: DEFAULT_QUEUE_DEPTH,
        })
        .unwrap();
        (
            router(AppState::new_with_artifact_runtime(
                agent,
                vec![],
                artifact_runtime.clone(),
            )),
            artifact_runtime,
        )
    })
    .await
    .unwrap();

    let (status, completed) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"lowered-login-3",
            "user_id":"lowered-user",
            "utterance":"123456",
            "channel":"web"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    assert!(!completed["suspended"].as_bool().unwrap());
    assert_eq!(calls.lock().await.len(), 2, "send and verify exactly once");
    tokio::task::spawn_blocking(move || drop((app, artifact_runtime)))
        .await
        .unwrap();
    server.abort();
}

#[tokio::test]
async fn catalog_lowering_rejects_raw_target_ids_before_artifact_admission() {
    let (app, artifact_runtime) = unified_app("catalog_lowering_raw_target");
    let mut catalog = World::demo_tenant("tenant-1").tenant;
    catalog.flows[0].lowering = Some(
        serde_json::from_value(json!({
            "format":1,
            "program":{"nid":"raw","op":"Call","id":"send_otp@1","args":{},"into":"sent"},
            "bindings":[{
                "name":"send","step_id":"collect_phone","capability":"auth.otp.send","deadline_ms":30000
            }],
            "cases":(0..20).map(|index| json!({
                "input":{"turn":{"case":index}},"expected":{"ok":true},"fixtures":[]
            })).collect::<Vec<_>>()
        }))
        .unwrap(),
    );
    let (status, rejected) = call(
        &app,
        "POST",
        "/v1/catalog",
        None,
        Some(serde_json::to_value(catalog).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{rejected}");
    assert!(artifact_runtime
        .artifact_repository()
        .unwrap()
        .get("tenant-1", "aelio.catalog.login", 1)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn unified_catalog_requires_exact_effect_and_refuses_unsupported_fallback_handoff() {
    let (app, artifact_runtime) = unified_app("catalog_lowering_closed_contract");
    let mut missing_effect = World::demo_tenant("tenant-1").tenant;
    missing_effect.tools[0].effect = None;
    let (status, rejected) = call(
        &app,
        "POST",
        "/v1/catalog",
        None,
        Some(serde_json::to_value(missing_effect).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");

    let mut fallback = World::demo_tenant("tenant-1").tenant;
    fallback.flows[0].escape = aelio_agent::tenant::FlowEscape::Fallback {
        flow_id: "recovery".into(),
    };
    fallback.flows[0].lowering = Some(
        serde_json::from_value(json!({
            "format":1,
            "program":{"nid":"reply","op":"Const","v":{"text":"ok"}},
            "bindings":[],
            "cases":(0..20).map(|index| json!({
                "input":{"case":index},"expected":{"text":"ok"},"fixtures":[]
            })).collect::<Vec<_>>()
        }))
        .unwrap(),
    );
    let (status, rejected) = call(
        &app,
        "POST",
        "/v1/catalog",
        None,
        Some(serde_json::to_value(fallback).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{rejected}");
    assert!(artifact_runtime
        .artifact_repository()
        .unwrap()
        .get("tenant-1", "aelio.catalog.login", 1)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn lowered_flow_owns_declared_escalation_when_a_pinned_tool_is_unavailable() {
    let (app, _artifact_runtime) = unified_app("catalog_lowering_escape");
    let mut catalog = World::demo_tenant("tenant-1").tenant;
    catalog.flows[0].lowering = Some(
        serde_json::from_value(json!({
            "format":1,
            "program":{"nid":"flow","op":"Seq","steps":[
                {"nid":"send","op":"Call","id":"$cap:send",
                 "args":{"args":{"pull":"turn"},"context":{"lit":{}}},"into":"sent"},
                {"nid":"done","op":"Const","v":{"text":"sent"}}
            ]},
            "bindings":[{
                "name":"send","step_id":"collect_phone","capability":"auth.otp.send",
                "deadline_ms":30000
            }],
            "cases":(0..20).map(|index| json!({
                "input":{"turn":{"case":index}},"expected":{"text":"sent"},
                "fixtures":[{"binding":"send","output":{"ok":true},"usage_tokens":0}]
            })).collect::<Vec<_>>()
        }))
        .unwrap(),
    );
    let (status, registered) = call(
        &app,
        "POST",
        "/v1/catalog",
        None,
        Some(serde_json::to_value(catalog).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{registered}");
    let (status, escaped) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"lowered-escape-1","user_id":"lowered-escape-user",
            "utterance":"log in","channel":"web"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{escaped}");
    assert!(!escaped["suspended"].as_bool().unwrap());
    assert_eq!(
        escaped["reply"]["text"],
        "I could not safely complete this flow. Human escalation is required."
    );
}

#[tokio::test]
async fn bound_authored_flow_parks_and_resumes_only_in_runtime_continuation() {
    let root = std::env::temp_dir().join(format!(
        "aelio_server_runtime_flow_e2e_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let agent_path = root.join("agent");
    std::fs::create_dir_all(&agent_path).unwrap();
    let store = AelioStore::new(Database::create(agent_path).unwrap(), 3).unwrap();
    let agent = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
    let artifact_runtime = Runtime::open(RuntimeConfig {
        data_dir: root.join("runtime"),
        host_token: None,
        host_url: None,
        event_key_secret: [47; 32],
        queue_depth: DEFAULT_QUEUE_DEPTH,
    })
    .unwrap();
    let flow = FlowPush {
        tenant: "tenant-1".into(),
        flow_id: "login.runtime".into(),
        flow_rev: "1".into(),
        program: json!({
            "nid":"login","op":"Seq","steps":[
                {"nid":"ask","op":"Const","v":{"text":"Enter the six-digit code"}},
                {"nid":"wait","op":"Park","until":{"kind":"event"},"into":"wake"},
                {"nid":"done","op":"Const","v":{"text":"Authenticated by runtime"}}
            ]
        }),
        targets: vec![],
        prompts: vec![],
    };
    artifact_runtime.push_flow(flow.clone()).unwrap();
    artifact_runtime
        .gate_flow(
            &flow,
            &(0..20)
                .map(|index| SandboxCase {
                    input: json!({"turn":{"case":index}}),
                    wakes: vec![],
                    expect_park: true,
                    expected: json!({"text":"Enter the six-digit code"}),
                    fixtures: vec![],
                })
                .collect::<Vec<_>>(),
            SandboxLimits::default(),
            Some("deployer:test".into()),
        )
        .unwrap();
    let record = artifact_runtime
        .artifact_repository()
        .unwrap()
        .get("tenant-1", "login.runtime", 1)
        .unwrap()
        .unwrap();
    let app = router(AppState::new_with_artifact_runtime(
        agent,
        vec!["admin-secret".into()],
        artifact_runtime.clone(),
    ));
    let mut catalog = serde_json::to_value(World::demo_tenant("tenant-1").tenant).unwrap();
    catalog["flow_artifacts"]["login"] = json!({
        "id":record.artifact.id,
        "version":record.artifact.version,
        "hash":record.artifact.hash,
    });
    let (status, binding) = call(
        &app,
        "POST",
        "/v1/catalog",
        Some("admin-secret"),
        Some(catalog),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{binding}");

    let first_request = json!({
        "turn_id":"runtime-login-1",
        "user_id":"runtime-login-user",
        "utterance":"log in",
        "channel":"web"
    });
    let (status, parked) = call(
        &app,
        "POST",
        "/v1/turns",
        Some("admin-secret"),
        Some(first_request),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{parked}");
    assert_eq!(parked["reply"]["text"], "Enter the six-digit code");
    assert_eq!(parked["suspended"], true);
    assert!(parked["active_flow"].is_null());
    assert!(parked["steps"].as_array().unwrap().iter().any(|step| {
        step["name"] == "ActivateFlow.Runtime"
            && step["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("authority=aelio-runtime"))
    }));
    assert!(artifact_runtime
        .active_subject_artifact("tenant-1", "runtime-login-user")
        .unwrap()
        .is_some());

    let second_request = json!({
        "turn_id":"runtime-login-2",
        "user_id":"runtime-login-user",
        "utterance":"123456",
        "channel":"web"
    });
    let (status, completed) = call(
        &app,
        "POST",
        "/v1/turns",
        Some("admin-secret"),
        Some(second_request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    assert_eq!(completed["reply"]["text"], "Authenticated by runtime");
    assert_eq!(completed["suspended"], false);
    assert!(completed["steps"].as_array().unwrap().iter().any(|step| {
        step["name"] == "FlowGate.Runtime"
            && step["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("authority=aelio-runtime"))
    }));
    assert!(artifact_runtime
        .active_subject_artifact("tenant-1", "runtime-login-user")
        .unwrap()
        .is_none());

    let (status, replay) = call(
        &app,
        "POST",
        "/v1/turns",
        Some("admin-secret"),
        Some(second_request),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["reply"], completed["reply"]);
}

#[tokio::test]
async fn rust_turn_api_executes_and_replays_a_durable_flow() {
    let app = app("turn", vec!["secret".into()]);
    let request = json!({
        "turn_id":"turn-1",
        "user_id":"u1",
        "utterance":"login with +919876543210"
    });
    let (status, first) = call(
        &app,
        "POST",
        "/v1/turns",
        Some("secret"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["suspended"], true);
    assert!(first["steps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|step| step["name"] == "Invoke.Call"));

    let (status, replay) = call(&app, "POST", "/v1/turns", Some("secret"), Some(request)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["reply"], first["reply"]);

    let (status, completed) = call(
        &app,
        "POST",
        "/v1/turns",
        Some("secret"),
        Some(json!({
            "turn_id":"turn-2",
            "user_id":"u1",
            "utterance":"434543"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    assert_eq!(completed["new_state"], "authenticated");
}

#[tokio::test]
async fn turn_api_requires_auth_but_health_does_not() {
    let protected_app = app("auth", vec!["secret".into()]);
    assert_eq!(
        call(&protected_app, "GET", "/v1/health", None, None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &protected_app,
            "POST",
            "/v1/turns",
            None,
            Some(json!({"turn_id":"t","user_id":"u","utterance":"Hi"})),
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    let open = app("open_admin", vec![]);
    assert_eq!(
        call(&open, "GET", "/v1/admin/flow-candidates", None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED,
        "open development mode must never open the learning control plane"
    );
}

#[tokio::test]
async fn oversized_http_bodies_are_rejected_before_json_deserialization() {
    let protected_app = app("body_limit", vec!["secret".into()]);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/turns")
        .header("authorization", "Bearer secret")
        .header("content-type", "application/json")
        .body(Body::from(vec![b' '; 4 * 1024 * 1024 + 1]))
        .unwrap();
    let response = protected_app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn production_bootstrap_cannot_serve_demo_behavior_before_catalog_registration() {
    let app = empty_app("unregistered", vec!["secret".into()]);
    let (status, body) = call(
        &app,
        "POST",
        "/v1/turns",
        Some("secret"),
        Some(json!({"turn_id":"t1","user_id":"u1","utterance":"login"})),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["code"], "unavailable");
}

#[tokio::test]
async fn runtime_credentials_cannot_reach_the_learning_control_plane() {
    let app = scoped_app("credential_scopes");
    let (health_status, health) = call(&app, "GET", "/v1/health", None, None).await;
    assert_eq!(health_status, StatusCode::OK);
    assert_eq!(health["ready"], false);
    assert_eq!(health["sdk_required"], true);
    assert_eq!(health["sdk_available"], false);
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/turns",
            Some("runtime-key"),
            Some(json!({"turn_id":"t1","user_id":"u1","utterance":"hi"})),
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE,
        "the runtime credential passed authentication, but production must fail closed until its SDK tool host connects"
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/v1/admin/procedures",
            Some("runtime-key"),
            None,
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", "/v1/admin/procedures", Some("admin-key"), None,)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn catalog_publication_is_validated_and_activated_atomically() {
    let app = app("catalog", vec![]);
    let (_, mut catalog) = call(&app, "GET", "/v1/catalog", None, None).await;
    let tools = catalog["tools"].as_array_mut().unwrap();
    tools.push(tools[0].clone());
    let (status, error) = call(&app, "POST", "/v1/catalog", None, Some(catalog)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["code"], "conflict");

    let (_, active) = call(&app, "GET", "/v1/catalog", None, None).await;
    let ids: Vec<_> = active["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids.iter().filter(|id| **id == "send_otp").count(),
        1,
        "invalid catalog must not become active"
    );

    let (_, mut invalid_flow_catalog) = call(&app, "GET", "/v1/catalog", None, None).await;
    invalid_flow_catalog["flows"][0]["steps"][0]["admissible"] = json!(["undeclared.capability"]);
    let (status, error) = call(
        &app,
        "POST",
        "/v1/catalog",
        None,
        Some(invalid_flow_catalog),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["code"], "validation");
}

#[tokio::test]
async fn unified_catalog_admits_every_tool_as_a_gated_runtime_proxy() {
    let (app, artifact_runtime) = unified_app("catalog_proxies");
    let (_, catalog) = call(&app, "GET", "/v1/catalog", None, None).await;
    let tools = catalog["tools"].as_array().unwrap().clone();

    let (status, response) = call(&app, "POST", "/v1/catalog", None, Some(catalog)).await;
    assert_eq!(status, StatusCode::OK, "{response}");

    let repository = artifact_runtime.artifact_repository().unwrap();
    for tool in tools {
        let id = tool["id"].as_str().unwrap();
        let version = tool["version"].as_str().unwrap().parse().unwrap();
        let proxy = repository
            .get("tenant-1", &format!("aelio.proxy.{id}"), version)
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                proxy.status,
                ArtifactStatus::Canary | ArtifactStatus::Promoted
            ),
            "proxy {id} was not admitted: {:?}",
            proxy.status
        );
        let names = proxy
            .artifact
            .interface
            .inputs
            .iter()
            .map(|input| input.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["args", "context"]);
    }
}

#[tokio::test]
async fn unmaterialized_public_flow_fails_closed_before_a_runtime_proxy_effect() {
    let calls = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let host = Router::new()
        .route("/internal/aelio/target", post(fake_host_target))
        .with_state(FakeHostState {
            calls: calls.clone(),
        });
    let server = tokio::spawn(async move { axum::serve(listener, host).await.unwrap() });
    let (app, artifact_runtime) = tokio::task::spawn_blocking(move || {
        unified_app_with_host("live_proxy", Some(format!("http://{address}")))
    })
    .await
    .unwrap();
    let (_, catalog) = call(&app, "GET", "/v1/catalog", None, None).await;
    let (status, registration) = call(&app, "POST", "/v1/catalog", None, Some(catalog)).await;
    assert_eq!(status, StatusCode::OK, "{registration}");

    let (status, turn) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"runtime-proxy-turn-1",
            "user_id":"u1",
            "utterance":"login with +919876543210"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{turn}");
    assert_eq!(turn["suspended"], false);
    let materialization_trace = turn["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "FlowMaterialization")
        .expect("unmaterialized flow must expose its closed materialization decision");
    assert!(materialization_trace["detail"]
        .as_str()
        .unwrap()
        .contains("demand recorded"));
    assert!(!serde_json::to_string(&turn)
        .unwrap()
        .contains("+919876543210"));
    let calls = calls.lock().await;
    assert_eq!(
        calls.len(),
        0,
        "an unmaterialized flow must not reach its otherwise-admitted tool proxy"
    );
    let demands = artifact_runtime
        .list_capability_requests("tenant-1", 10)
        .unwrap();
    assert!(demands.iter().any(|demand| demand
        .draft
        .normalized_need
        .contains("materialize authored flow login@1")));
    drop(calls);
    server.abort();
    tokio::task::spawn_blocking(move || drop((app, artifact_runtime)))
        .await
        .unwrap();
}

#[tokio::test]
async fn materialized_public_flow_owns_effect_and_continuation_end_to_end() {
    let calls = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let host = Router::new()
        .route("/internal/aelio/target", post(fake_host_target))
        .with_state(FakeHostState {
            calls: calls.clone(),
        });
    let server = tokio::spawn(async move { axum::serve(listener, host).await.unwrap() });

    let root = std::env::temp_dir().join(format!(
        "aelio_server_materialized_effect_{}",
        std::process::id()
    ));
    let setup_root = root.clone();
    let (app, artifact_runtime, artifact_pin) = tokio::task::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(&setup_root);
        std::fs::create_dir_all(setup_root.join("agent")).unwrap();
        let store =
            AelioStore::new(Database::create(setup_root.join("agent")).unwrap(), 3).unwrap();
        let artifact_runtime = Runtime::open(RuntimeConfig {
            data_dir: setup_root.join("runtime"),
            host_token: Some("test-host-token".into()),
            host_url: Some(format!("http://{address}")),
            event_key_secret: [53; 32],
            queue_depth: DEFAULT_QUEUE_DEPTH,
        })
        .unwrap();
        let flow = FlowPush {
            tenant: "tenant-1".into(),
            flow_id: "login.runtime.effect".into(),
            flow_rev: "1".into(),
            program: json!({
                "nid":"phone_gate","op":"Branch",
                "pred":{"fn":"contains","args":[
                    {"pull":"turn.utterance"},{"lit":"+"}
                ]},
                "then":{"nid":"login","op":"Seq","steps":[
                    {"nid":"send_once","op":"Once","body":{
                        "nid":"send","op":"Call","id":"send_otp@1",
                        "args":{
                            "args":{"pull":"turn"},
                            "context":{"lit":{"source":"aelio.runtime.flow"}}
                        },
                        "into":"sent"
                    }},
                    {"nid":"ask_first","op":"Const","v":{"text":"Enter the six-digit code"}},
                    {"nid":"wait_first","op":"Park","until":{"kind":"event"},"into":"otp_first"},
                    {"nid":"check_first","op":"Branch",
                     "pred":{"fn":"eq","args":[
                         {"pull":"otp_first.turn.utterance"},{"lit":"123456"}
                     ]},
                     "then":{"nid":"done_first","op":"Const","v":{"text":"Authenticated by runtime"}},
                     "else":{"nid":"repair","op":"Seq","steps":[
                         {"nid":"ask_again","op":"Const","v":{"text":"Enter the six-digit code"}},
                         {"nid":"wait_again","op":"Park","until":{"kind":"event"},"into":"otp_second"},
                         {"nid":"check_second","op":"Branch",
                          "pred":{"fn":"eq","args":[
                              {"pull":"otp_second.turn.utterance"},{"lit":"123456"}
                          ]},
                          "then":{"nid":"done_second","op":"Const","v":{"text":"Authenticated by runtime"}},
                          "else":{"nid":"failed","op":"Const","v":{"text":"OTP verification failed"}}}
                     ]}}
                ]},
                "else":{"nid":"invalid_phone","op":"Const","v":{
                    "text":"Please provide a valid international phone number"
                }}
            }),
            targets: vec![TargetSpec {
                id: "send_otp@1".into(),
                class: TargetClassSpec::Tool,
                effect: EffectSpec::External,
                input_imprint: "aelio.turn.input@1".into(),
                output_imprint: "aelio.turn.output@1".into(),
                bounded: BoundSpec::Deadline { max_ms: 30_000 },
                policy_tags: vec!["auth.send_otp".into()],
                origin: OriginSpec::Tenant,
            }],
            prompts: vec![],
        };
        artifact_runtime.push_flow(flow.clone()).unwrap();
        artifact_runtime
            .gate_flow(
                &flow,
                &(0..20)
                    .map(|index| SandboxCase {
                        input: json!({"turn":{"utterance":if index < 5 {
                            format!("login invalid {index}")
                        } else {
                            format!("login +9198765432{index:02}")
                        }}}),
                        wakes: if index < 5 { vec![] } else { vec![
                            json!({"turn":{"utterance":"000000"}}),
                            json!({"turn":{"utterance":"123456"}}),
                        ]},
                        expect_park: false,
                        expected: if index < 5 {
                            json!({"text":"Please provide a valid international phone number"})
                        } else {
                            json!({"text":"Authenticated by runtime"})
                        },
                        fixtures: if index < 5 { vec![] } else { vec![SandboxFixtureCall {
                            target: "send_otp@1".into(),
                            expected_args_hash: None,
                            output: json!({"ok":true,"continuation":"auth.otp.verify"}),
                            usage_tokens: 0,
                        }]},
                    })
                    .collect::<Vec<_>>(),
                SandboxLimits::default(),
                Some("deployer:test".into()),
            )
            .unwrap();
        let record = artifact_runtime
            .artifact_repository()
            .unwrap()
            .get("tenant-1", "login.runtime.effect", 1)
            .unwrap()
            .unwrap();
        let login_pin = aelio_agent::adaptive::ArtifactPinV1 {
            id: record.artifact.id,
            version: record.artifact.version,
            hash: record.artifact.hash,
        };
        let artifact_pin = json!({
            "id":login_pin.id,
            "version":login_pin.version,
            "hash":login_pin.hash,
        });

        let detour_flow = FlowPush {
            tenant: "tenant-1".into(),
            flow_id: "procedure.detour.boundary".into(),
            flow_rev: "1".into(),
            program: json!({
                "nid":"detour_reply","op":"Const","v":{"text":"I can answer account questions while login remains parked"}
            }),
            targets: vec![],
            prompts: vec![],
        };
        artifact_runtime.push_flow(detour_flow.clone()).unwrap();
        artifact_runtime
            .gate_flow(
                &detour_flow,
                &(0..20)
                    .map(|index| SandboxCase {
                        input: json!({"turn":{"case":index}}),
                        wakes: vec![],
                        expect_park: false,
                        expected: json!({"text":"I can answer account questions while login remains parked"}),
                        fixtures: vec![],
                    })
                    .collect::<Vec<_>>(),
                SandboxLimits::default(),
                Some("deployer:test".into()),
            )
            .unwrap();
        let detour_record = artifact_runtime
            .artifact_repository()
            .unwrap()
            .get("tenant-1", "procedure.detour.boundary", 1)
            .unwrap()
            .unwrap();
        let mut world = World::demo_tenant("tenant-1");
        let mut catalog = world.tenant.clone();
        catalog.flow_artifacts.insert("login".into(), login_pin);
        world.replace_tenant(catalog).unwrap();
        let detour_filter = aelio_agent::abilities::registry::SituationFilter {
            state: Some("unauthenticated".into()),
            intent_class: Some("boundary".into()),
            required_slots: vec![],
            capability_tags: world.tenant.states[0].permission_envelope.clone(),
        };
        let detour_embedding = aelio_agent::abilities::learn::embed_situation_filter(
            &aelio_agent::embedding::HashEmbedder::new(3).unwrap(),
            &detour_filter,
        )
        .unwrap();
        let detour_procedure_id = "procedure-detour-boundary-v1";
        world.registry.register_procedure(
            aelio_agent::abilities::registry::ProcedureSpec {
                id: detour_procedure_id.into(),
                version: "1".into(),
                tenant_id: "tenant-1".into(),
                situation_hash: "detour-tier-one".into(),
                situation_filter: detour_filter,
                situation_embedding: detour_embedding,
                path: aelio_agent::contract::AbilityPath::seq(["Express.Template"]),
                contract: world.registry.abilities["Express.Template"].clone(),
                tool_deps: vec![],
                prompt_deps: vec![],
                evidence: aelio_agent::abilities::registry::ProcedureEvidence {
                    observations: 20,
                    success_rate: 1.0,
                    mean_cost: 0.0,
                    mean_latency_ms: 1.0,
                },
                status: aelio_agent::abilities::registry::ProcedureStatus::Promoted,
                provenance: aelio_agent::abilities::registry::ProcedureProvenance {
                    origin: "test".into(),
                    proposed_by: "test".into(),
                    approved_by: Some("test".into()),
                },
                supersedes: None,
            },
        );
        world
            .registry
            .bind_procedure_artifact(
                detour_procedure_id,
                aelio_agent::adaptive::ArtifactPinV1 {
                    id: detour_record.artifact.id,
                    version: detour_record.artifact.version,
                    hash: detour_record.artifact.hash,
                },
            )
            .unwrap();
        let agent = DurableRuntime::new(world, store).unwrap();
        let app = router(AppState::new_with_artifact_runtime(
            agent,
            vec![],
            artifact_runtime.clone(),
        ));
        (app, artifact_runtime, artifact_pin)
    })
    .await
    .unwrap();
    assert_eq!(artifact_pin["id"], "login.runtime.effect");

    let (status, invalid) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-invalid",
            "user_id":"materialized-user",
            "utterance":"login with an invalid number"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{invalid}");
    assert_eq!(invalid["suspended"], false);
    assert_eq!(
        invalid["reply"]["text"],
        "Please provide a valid international phone number"
    );
    assert!(
        calls.lock().await.is_empty(),
        "invalid input emitted an effect"
    );

    let (status, parked) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-1",
            "user_id":"materialized-user",
            "utterance":"login with +919876543210"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{parked}");
    assert_eq!(parked["suspended"], true);
    assert_eq!(parked["reply"]["text"], "Enter the six-digit code");
    assert!(parked["steps"].as_array().unwrap().iter().any(|step| {
        step["name"] == "ActivateFlow.Runtime"
            && step["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("authority=aelio-runtime"))
    }));
    let ledger = parked["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Artifact.Ledger")
        .expect("bound flow must hydrate the authoritative runtime ledger");
    let ledger: Value = serde_json::from_str(ledger["detail"].as_str().unwrap()).unwrap();
    assert_eq!(ledger["artifact"], "login.runtime.effect@1");
    assert_eq!(ledger["ledger_hash"].as_str().unwrap().len(), 64);
    let ledger_steps = ledger["steps"].as_array().unwrap();
    assert!(!ledger_steps.is_empty());
    assert!(ledger_steps
        .windows(2)
        .all(|pair| { pair[0]["seq"].as_u64().unwrap() < pair[1]["seq"].as_u64().unwrap() }));
    let kinds = ledger_steps
        .iter()
        .map(|step| step["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    let intent = kinds
        .iter()
        .position(|kind| *kind == "call_intent")
        .unwrap();
    let dispatch = kinds
        .iter()
        .position(|kind| *kind == "call_dispatch")
        .unwrap();
    let result = kinds
        .iter()
        .position(|kind| *kind == "call_result")
        .unwrap();
    assert!(intent < dispatch && dispatch < result);
    assert!(!serde_json::to_string(&ledger)
        .unwrap()
        .contains("+919876543210"));
    let (continuation, continuation_version) = artifact_runtime
        .active_subject_artifact("tenant-1", "materialized-user")
        .unwrap()
        .expect("park must publish the subject continuation index");
    assert_eq!(continuation.artifact_id, "login.runtime.effect");
    assert_eq!(continuation.artifact_version, 1);
    assert_eq!(
        continuation.status,
        aelio_runtime::SubjectContinuationStatus::Active
    );
    let recorded_calls = calls.lock().await;
    assert_eq!(
        recorded_calls.len(),
        1,
        "the Once rail must emit one logical effect"
    );
    assert_eq!(recorded_calls[0]["target"], "send_otp@1");
    assert_eq!(recorded_calls[0]["tenant"], "tenant-1");
    assert_eq!(
        recorded_calls[0]["args"]["args"]["utterance"],
        "login with +919876543210"
    );
    drop(recorded_calls);

    let (status, detour) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-detour",
            "user_id":"materialized-user",
            "utterance":"what can you do?"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detour}");
    assert_eq!(detour["suspended"], false, "{detour}");
    assert_eq!(
        detour["reply"]["text"], "I can answer account questions while login remains parked",
        "{detour}"
    );
    assert!(detour["steps"].as_array().unwrap().iter().any(|step| {
        step["name"] == "FlowGate.Detour"
            && step["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("continuation unchanged"))
    }));
    let detour_ledger = detour["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["name"] == "Artifact.Ledger")
        .expect("the read-only detour must execute its exact admitted artifact");
    let detour_ledger: Value =
        serde_json::from_str(detour_ledger["detail"].as_str().unwrap()).unwrap();
    assert_eq!(detour_ledger["artifact"], "procedure.detour.boundary@1");
    assert_eq!(
        calls.lock().await.len(),
        1,
        "read-only detour emitted an effect"
    );
    let (after_detour, after_detour_version) = artifact_runtime
        .active_subject_artifact("tenant-1", "materialized-user")
        .unwrap()
        .expect("detour must leave the original continuation parked");
    assert_eq!(after_detour, continuation);
    assert_eq!(after_detour_version, continuation_version);
    let (status, detour_replay) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-detour",
            "user_id":"materialized-user",
            "utterance":"what can you do?"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detour_replay}");
    assert_eq!(detour_replay, detour);
    assert_eq!(calls.lock().await.len(), 1);

    let (status, deferred) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-deferred",
            "user_id":"materialized-user",
            "utterance":"login with +919876543299"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deferred}");
    assert_eq!(deferred["suspended"], true);
    assert!(deferred["steps"].as_array().unwrap().iter().any(|step| {
        step["name"] == "FlowGate.Defer"
            && step["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("continuation unchanged"))
    }));
    assert!(deferred["steps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|step| step["name"] == "Memory.OpenLoop"));
    assert_eq!(
        calls.lock().await.len(),
        1,
        "a deferred second flow must not emit an effect"
    );
    let (after_defer, after_defer_version) = artifact_runtime
        .active_subject_artifact("tenant-1", "materialized-user")
        .unwrap()
        .expect("deferral must preserve the original subject continuation");
    assert_eq!(after_defer, continuation);
    assert_eq!(after_defer_version, continuation_version);

    let (status, deferred_replay) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-deferred",
            "user_id":"materialized-user",
            "utterance":"login with +919876543299"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deferred_replay}");
    assert_eq!(deferred_replay, deferred);
    assert_eq!(calls.lock().await.len(), 1);

    let (status, repaired) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-2",
            "user_id":"materialized-user",
            "utterance":"000000"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{repaired}");
    assert_eq!(repaired["suspended"], true);
    assert_eq!(repaired["reply"]["text"], "Enter the six-digit code");
    assert!(repaired["steps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|step| step["name"] == "Artifact.Ledger"));
    assert_eq!(
        calls.lock().await.len(),
        1,
        "repair must not resend the OTP"
    );

    // Reopen both durable authorities between repair and completion. The active subject index,
    // exact artifact pin, program counter and Once ledger must survive without agent-owned flow
    // state or another side effect.
    tokio::task::spawn_blocking(move || drop((app, artifact_runtime)))
        .await
        .unwrap();
    let (app, artifact_runtime, root) = tokio::task::spawn_blocking(move || {
        let store = AelioStore::new(Database::open(root.join("agent")).unwrap(), 3).unwrap();
        let agent = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
        let open_loops = agent.list_open_loops(10).unwrap();
        assert_eq!(open_loops.len(), 1);
        assert_eq!(
            open_loops[0].envelope.value.open_loop_state.as_deref(),
            Some("deferred")
        );
        assert!(!open_loops[0].envelope.value.text.contains("+919876543299"));
        let artifact_runtime = Runtime::open(RuntimeConfig {
            data_dir: root.join("runtime"),
            host_token: Some("test-host-token".into()),
            host_url: Some(format!("http://{address}")),
            event_key_secret: [53; 32],
            queue_depth: DEFAULT_QUEUE_DEPTH,
        })
        .unwrap();
        let app = router(AppState::new_with_artifact_runtime(
            agent,
            vec![],
            artifact_runtime.clone(),
        ));
        (app, artifact_runtime, root)
    })
    .await
    .unwrap();

    let (status, completed) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-3",
            "user_id":"materialized-user",
            "utterance":"123456"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    assert_eq!(completed["suspended"], false);
    assert!(completed["reply"]["text"].as_str().is_some_and(
        |text| text.starts_with("Authenticated by runtime I also kept your deferred request:")
    ));
    assert!(!completed["reply"]["text"]
        .as_str()
        .unwrap()
        .contains("+919876543299"));
    assert!(completed["steps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|step| step["name"] == "Artifact.Ledger"));
    assert!(completed["steps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|step| step["name"] == "Memory.OpenLoopReady"));
    assert!(artifact_runtime
        .active_subject_artifact("tenant-1", "materialized-user")
        .unwrap()
        .is_none());
    assert_eq!(
        calls.lock().await.len(),
        1,
        "resume must not resend the OTP"
    );

    let (status, replay) = call(
        &app,
        "POST",
        "/v1/turns",
        None,
        Some(json!({
            "turn_id":"materialized-login-3",
            "user_id":"materialized-user",
            "utterance":"123456"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["reply"], completed["reply"]);
    assert_eq!(calls.lock().await.len(), 1, "replay repeated an effect");

    server.abort();
    tokio::task::spawn_blocking(move || {
        drop((app, artifact_runtime));
        let store = AelioStore::new(Database::open(root.join("agent")).unwrap(), 3).unwrap();
        let agent = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
        let open_loops = agent.list_open_loops(10).unwrap();
        assert_eq!(open_loops.len(), 1);
        assert_eq!(
            open_loops[0].envelope.value.open_loop_state.as_deref(),
            Some("ready")
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn admin_worker_tick_leases_due_jobs() {
    let app = app("worker", vec!["secret".into()]);
    let (status, scheduled) = call(
        &app,
        "POST",
        "/v1/admin/jobs",
        Some("secret"),
        Some(json!({
            "id": "job-1",
            "kind": "park_resume",
            "payload": {"turn_id": "turn-1"},
            "state": "scheduled",
            "scheduled_at_ms": 10,
            "owner": null,
            "lease_expires_at_ms": null,
            "attempts": 0,
            "max_attempts": 3,
            "last_error": null,
            "terminal": null
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{scheduled}");
    assert_eq!(scheduled["scheduled"], true);

    let (status, tick) = call(
        &app,
        "POST",
        "/v1/admin/workers/tick",
        Some("secret"),
        Some(json!({
            "owner": "worker-1",
            "now_ms": 10,
            "lease_ms": 1000,
            "limit": 10
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tick}");
    assert_eq!(tick["leased_jobs"][0]["id"], "job-1");
    assert_eq!(tick["leased_jobs"][0]["owner"], "worker-1");

    let (status, completed) = call(
        &app,
        "POST",
        "/v1/admin/workers/complete",
        Some("secret"),
        Some(json!({
            "owner": "worker-1",
            "job_id": "job-1",
            "now_ms": 11
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    assert_eq!(completed["completed"], true);

    let (_, tick) = call(
        &app,
        "POST",
        "/v1/admin/workers/tick",
        Some("secret"),
        Some(json!({
            "owner": "worker-2",
            "now_ms": 12,
            "lease_ms": 1000,
            "limit": 10
        })),
    )
    .await;
    assert_eq!(tick["leased_jobs"], json!([]));
}

#[tokio::test]
async fn proactive_api_applies_deterministic_gates_before_enqueue() {
    let app = app("proactive", vec!["secret".into()]);
    let request = |opted_in| {
        json!({
            "candidate": {
                "user_id": "u1",
                "fingerprint": "open-loop-1",
                "payload": {"message": "follow up"},
                "opted_in": opted_in,
                "deterministic_gate": true,
                "confidence_millis": 900,
                "min_confidence_millis": 800
            },
            "policy": {
                "min_cadence_ms": 1000,
                "max_enqueues_per_day": 1,
                "suppression_ms": 500,
                "max_job_attempts": 2
            },
            "now_ms": 10000
        })
    };
    let (status, suppressed) = call(
        &app,
        "POST",
        "/v1/proactive/evaluate",
        Some("secret"),
        Some(request(false)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{suppressed}");
    assert_eq!(suppressed["suppressed"]["reason"], "not_opted_in");

    let (status, enqueued) = call(
        &app,
        "POST",
        "/v1/proactive/evaluate",
        Some("secret"),
        Some(request(true)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{enqueued}");
    assert!(enqueued["enqueued"]["job_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("proactive:")));
}

#[tokio::test]
async fn exploration_policy_is_authenticated_and_rejects_unbounded_configuration() {
    let app = app("exploration_policy", vec!["secret".into()]);
    let valid = json!({
        "enabled": true,
        "sample_rate_bps": 500,
        "max_trials_per_window": 10,
        "window_ms": 3_600_000,
        "max_candidate_steps": 8
    });
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/admin/exploration",
            None,
            Some(valid.clone()),
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, configured) = call(
        &app,
        "POST",
        "/v1/admin/exploration",
        Some("secret"),
        Some(valid),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{configured}");
    assert_eq!(configured["sample_rate_bps"], 500);

    let (status, error) = call(
        &app,
        "POST",
        "/v1/admin/exploration",
        Some("secret"),
        Some(json!({
            "enabled": true,
            "sample_rate_bps": 10_001,
            "max_trials_per_window": 0,
            "window_ms": 0,
            "max_candidate_steps": 0
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["code"], "validation");
}

#[tokio::test]
async fn candidate_flow_review_surface_is_authenticated_and_bounded() {
    let app = app("candidate_flow_api", vec!["secret".into()]);
    assert_eq!(
        call(&app, "GET", "/v1/admin/flow-candidates", None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, listed) = call(
        &app,
        "GET",
        "/v1/admin/flow-candidates?limit=10",
        Some("secret"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["items"], json!([]));

    let (status, error) = call(
        &app,
        "GET",
        "/v1/admin/flow-candidates?limit=1001",
        Some("secret"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{error}");
    assert_eq!(error["code"], "budget_exceeded");
}

#[tokio::test]
async fn learning_control_plane_can_inspect_and_kill_a_promoted_procedure() {
    let app = app("learning_admin", vec!["secret".into()]);
    for index in 0..5 {
        let (status, body) = call(
            &app,
            "POST",
            "/v1/turns",
            Some("secret"),
            Some(json!({
                "turn_id": format!("learn-{index}"),
                "user_id": format!("user-{index}"),
                "utterance": "hi"
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let (status, procedures) = call(
        &app,
        "GET",
        "/v1/admin/procedures?limit=10",
        Some("secret"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{procedures}");
    let procedure_id = procedures["items"][0]["procedure"]["procedure_id"]
        .as_str()
        .unwrap();
    assert_eq!(procedures["items"][0]["status"], "active");

    let (status, suspended) = call(
        &app,
        "POST",
        &format!("/v1/admin/procedures/{procedure_id}/suspend"),
        Some("secret"),
        Some(json!({"reason": "operator kill switch test"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{suspended}");
    assert_eq!(suspended["versions"], 1);

    let (_, procedures) = call(
        &app,
        "GET",
        "/v1/admin/procedures?limit=10",
        Some("secret"),
        None,
    )
    .await;
    assert_eq!(procedures["items"][0]["status"], "suspended");
}
