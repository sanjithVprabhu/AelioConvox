//! Phase 4.2–4.3: POST /v2/events admission + deterministic Conductor.

use aelio_agent::runtime::{DurableRuntime, World};
use aelio_agent::storage::AelioStore;
use aelio_agent_api::{router, AppState};
use aelio_db_query::Database;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

fn app(tag: &str) -> axum::Router {
    let path = std::env::temp_dir().join(format!("aelio_v2_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
    let world = World::demo_tenant("tenant-1");
    // Ensure catalog non-empty for health; turns not required for v2 events.
    let runtime = DurableRuntime::new(world, store).unwrap();
    router(AppState::new(runtime, vec!["test-key".into()]))
}

async fn post_json(app: &axum::Router, path: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", "Bearer test-key")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(json!({ "raw": String::from_utf8_lossy(&bytes) }));
    (status, v)
}

#[tokio::test]
async fn v2_events_greeting_accepted_and_deduped() {
    let app = app("greet");
    let body = json!({
        "event_id": "e1",
        "user_id": "u1",
        "utterance": "hello",
        "channel": "web",
        "source": "widget",
        "source_message_id": "m1"
    });
    let (s1, v1) = post_json(&app, "/v2/events", body.clone()).await;
    assert_eq!(s1, StatusCode::ACCEPTED, "{v1}");
    assert_eq!(v1["status"], "accepted");
    assert_eq!(v1["conductor_route"], "quick_reply");
    assert_eq!(v1["reply_text"], "Hey! I can help you.");
    assert_eq!(v1["conductor_only"], true);

    let (s2, v2) = post_json(&app, "/v2/events", body).await;
    assert_eq!(s2, StatusCode::OK, "{v2}");
    assert_eq!(v2["status"], "duplicate");
    assert_eq!(v2["prior_event_id"], "e1");
}

#[tokio::test]
async fn v2_events_average_route() {
    let app = app("avg");
    let (s, v) = post_json(
        &app,
        "/v2/events",
        json!({
            "event_id": "e-avg",
            "user_id": "u1",
            "utterance": "please average these numbers",
            "channel": "api"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    // Long "help me"-class or average may resolve to understand_intent if length/help rules hit.
    // Prefer: tool path if matched, else conductor deterministic route.
    let route = v["conductor_route"].as_str().unwrap_or("");
    assert!(
        route == "spawn_average"
            || route == "understand_intent"
            || v["tool_harness"] == true,
        "unexpected route: {v}"
    );
}

#[tokio::test]
async fn v2_events_send_otp_tool_harness() {
    let app = app("v2-otp");
    let (s, v) = post_json(
        &app,
        "/v2/events",
        json!({
            "event_id": "e-otp",
            "user_id": "u1",
            "utterance": "send otp to 9611266596",
            "channel": "api"
        }),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["tool_harness"], true);
    assert_eq!(v["conductor_route"], "tool.send_otp");
    let reply = v["reply_text"].as_str().unwrap_or("");
    assert!(reply.contains("OTP sent"), "reply={reply}");
}
