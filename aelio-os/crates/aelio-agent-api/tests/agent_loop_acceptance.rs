use aelio_agent::runtime::{DurableRuntime, World};
use aelio_agent::storage::AelioStore;
use aelio_agent_api::{router, AppState};
use aelio_db_query::Database;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

#[derive(Clone)]
struct GatewayScript(Arc<AtomicUsize>);

async fn capabilities() -> Json<Value> {
    Json(json!({
        "protocol_version": 2,
        "provider": "scripted-test",
        "model": "agent-loop-acceptance",
        "native_tools": true,
        "prompt_caching": false,
        "streaming": false,
        "max_output_tokens": 4096
    }))
}

async fn complete(State(script): State<GatewayScript>, Json(request): Json<Value>) -> Json<Value> {
    let sequence = script.0.fetch_add(1, Ordering::SeqCst);
    if sequence == 7 {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    let tool_call = |id: &str, name: &str, arguments: Value| {
        json!({
            "type": "tool_call",
            "call": {"id": id, "name": name, "arguments": arguments}
        })
    };
    let finish = |id: &str, message: &str| {
        tool_call(
            id,
            "finish",
            json!({
                "message": message,
                "status": "completed",
                "resolved_effect_ids": [],
                "unresolved_effect_ids": []
            }),
        )
    };
    let content = match sequence {
        0 => vec![tool_call(
            "missing-phone",
            "send_otp",
            json!({"tenant_id":"tenant-1"}),
        )],
        1 => vec![tool_call(
            "send-complete",
            "send_otp",
            json!({"tenant_id":"tenant-1","phone":"+919611266596"}),
        )],
        2 => vec![finish("send-finish", "The code was sent.")],
        3 => vec![tool_call(
            "verify-complete",
            "verify_otp",
            json!({"phone":"+919611266596","otp":"434543"}),
        )],
        4 => vec![finish("verify-finish", "You are signed in.")],
        5 => vec![tool_call(
            "clients-after-login",
            "clients_query",
            json!({"sort_by":"generosity_index","order":"desc","limit":3}),
        )],
        6 => vec![finish("clients-finish", "I found the client records.")],
        7 => vec![finish(
            "late-finish",
            "This reply must be replaced by cancellation.",
        )],
        other => panic!("unexpected gateway call {other}"),
    };
    Json(json!({
        "protocol_version": 2,
        "request_id": request["request_id"],
        "attempt_id": request["attempt_id"],
        "response": {
            "id": format!("scripted-response-{sequence}"),
            "content": content,
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 10,
                "cached_input_tokens": 0,
                "output_tokens": 5
            }
        }
    }))
}

async fn post_turn(app: &Router, turn_id: &str, utterance: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/turns")
                .header("authorization", "Bearer test-key")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "turn_id": turn_id,
                        "user_id": "agent-loop-user",
                        "utterance": utterance,
                        "channel": "web"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn missing_input_confirmation_continuation_and_state_transition_are_end_to_end() {
    let script = GatewayScript(Arc::new(AtomicUsize::new(0)));
    let gateway = Router::new()
        .route("/agent/capabilities", get(capabilities))
        .route("/agent", post(complete))
        .with_state(script.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway_task = tokio::spawn(async move {
        axum::serve(listener, gateway).await.unwrap();
    });

    std::env::set_var("AELIO_HARNESS_MODE", "agent_loop");
    std::env::set_var(
        "AELIO_AGENT_LOOP_GATEWAY_URL",
        format!("http://{address}/agent"),
    );
    std::env::set_var("AELIO_LLM_GATEWAY_TOKEN", "test-gateway-token");
    std::env::set_var("AELIO_AGENT_LOOP_STATE_KEY", "5a".repeat(32));

    let directory = tempfile::tempdir().unwrap();
    let store = AelioStore::new(Database::create(directory.path()).unwrap(), 3).unwrap();
    let runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
    let app = router(AppState::new(runtime, vec!["test-key".to_string()]));

    let (status, missing) = post_turn(&app, "turn-1", "send me a login code").await;
    assert_eq!(status, StatusCode::OK, "{missing}");
    assert_eq!(missing["suspended"], true);
    assert!(missing["reply"]["text"].as_str().unwrap().contains("phone"));
    assert!(missing["steps"][1]["detail"]
        .as_str()
        .unwrap()
        .contains("tool_calls=0"));

    let (_, confirmation) = post_turn(&app, "turn-2", "use +919611266596").await;
    assert_eq!(confirmation["suspended"], true, "{confirmation}");
    let (_, sent) = post_turn(&app, "turn-3", "yes").await;
    assert_eq!(sent["suspended"], false, "{sent}");

    let (_, verify_confirmation) = post_turn(&app, "turn-4", "434543").await;
    assert_eq!(
        verify_confirmation["suspended"], true,
        "{verify_confirmation}"
    );
    let (_, verified) = post_turn(&app, "turn-5", "yes").await;
    assert_eq!(verified["new_state"], "authenticated", "{verified}");

    let (_, clients) = post_turn(&app, "turn-6", "show the clients").await;
    assert_eq!(clients["suspended"], false, "{clients}");
    assert!(clients["reply"]["text"]
        .as_str()
        .unwrap()
        .contains("client records"));
    assert_eq!(script.0.load(Ordering::SeqCst), 7);

    let long_app = app.clone();
    let long_turn = tokio::spawn(async move {
        post_turn(&long_app, "turn-7", "take your time answering this").await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (cancel_status, cancel_ack) = post_turn(&app, "turn-8", "stop this run").await;
    assert_eq!(cancel_status, StatusCode::OK, "{cancel_ack}");
    assert!(cancel_ack["reply"]["text"]
        .as_str()
        .unwrap()
        .contains("safe stop"));
    let (long_status, stopped) = long_turn.await.unwrap();
    assert_eq!(long_status, StatusCode::OK, "{stopped}");
    assert!(stopped["reply"]["text"]
        .as_str()
        .unwrap()
        .contains("stopped this request"));
    assert_eq!(script.0.load(Ordering::SeqCst), 8);

    gateway_task.abort();
}
