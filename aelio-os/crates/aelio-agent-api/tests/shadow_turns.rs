//! Phase 4.4: shadow Conductor observation on live /v1/turns (agent remains authority).

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
    let path = std::env::temp_dir().join(format!("aelio_shadow_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
    let world = World::demo_tenant("tenant-1");
    let runtime = DurableRuntime::new(world, store).unwrap();
    router(AppState::new(runtime, vec!["test-key".into()]))
}

async fn post_turn(
    app: &axum::Router,
    turn_id: &str,
    utterance: &str,
) -> (StatusCode, serde_json::Value) {
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
                        "user_id": "u1",
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(json!({ "raw": String::from_utf8_lossy(&bytes) }));
    (status, v)
}

fn shadow_detail(v: &serde_json::Value) -> Option<&str> {
    v["steps"].as_array()?.iter().find_map(|s| {
        if s["name"] == "Conductor.Shadow" {
            s["detail"].as_str()
        } else {
            None
        }
    })
}

#[tokio::test]
async fn shadow_greeting_agrees_quick_reply() {
    let app = app("hi");
    let (status, v) = post_turn(&app, "t-hi", "hello").await;
    assert_eq!(status, StatusCode::OK, "{v}");
    // Phase 4.5: pure greets are cut over to conductor.root (authority = OS).
    let steps = v["steps"].as_array().expect("steps");
    let cutover = steps.iter().any(|s| s["name"] == "Conductor.Cutover");
    assert!(cutover, "expected Conductor.Cutover for hello: {v}");
    let detail = shadow_detail(&v).expect("Conductor.Shadow step");
    assert!(
        detail.contains("agree=true") && detail.contains("cutover=true"),
        "unexpected shadow detail: {detail}"
    );
    assert_eq!(v["llm_calls"], 0);
    assert!(!v["reply"]["text"].as_str().unwrap_or("").is_empty());
}

#[tokio::test]
async fn help_me_uses_sol_understand_not_agent_spine() {
    let app = app("help2");
    let (status, v) = post_turn(&app, "t-help2", "help me figure out what to do next").await;
    assert_eq!(status, StatusCode::OK, "{v}");
    // Phase 6 dual-IR reduction: installed understand_intent Sol harness owns this path.
    assert!(
        step_detail_contains(&v, "Conductor.Select", "understand_intent")
            || shadow_detail(&v).is_some_and(|d| d.contains("understand_intent")),
        "expected understand_intent path: {v}"
    );
    assert!(!step_named(&v, "ProposePath"));
    let detail = shadow_detail(&v).unwrap_or("");
    assert!(
        detail.contains("tool_cutover=true") || detail.contains("understand_intent"),
        "unexpected shadow detail: {detail}"
    );
}

fn step_named(v: &serde_json::Value, name: &str) -> bool {
    v["steps"]
        .as_array()
        .map(|steps| steps.iter().any(|s| s["name"] == name))
        .unwrap_or(false)
}

fn step_detail_contains(v: &serde_json::Value, name: &str, needle: &str) -> bool {
    v["steps"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .any(|s| s["name"] == name && s["detail"].as_str().unwrap_or("").contains(needle))
        })
        .unwrap_or(false)
}
