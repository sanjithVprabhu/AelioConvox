//! Phase 5: installed tool harness cutover on /v1/turns (no cold ProposePath).

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
    let path = std::env::temp_dir().join(format!("aelio_tool_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    let store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
    let world = World::demo_tenant("tenant-1");
    let runtime = DurableRuntime::new(world, store).unwrap();
    router(AppState::new(runtime, vec!["test-key".into()]))
}

async fn post_turn(app: &axum::Router, turn_id: &str, utterance: &str) -> (StatusCode, serde_json::Value) {
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

#[tokio::test]
async fn send_otp_uses_installed_tool_harness_not_propose_path() {
    let app = app("otp");
    let (status, v) = post_turn(&app, "t-otp", "send otp to 9611266596").await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(
        step_named(&v, "Harness.Tool"),
        "expected Harness.Tool step: {v}"
    );
    assert!(
        step_detail_contains(&v, "Conductor.Select", "tool.send_otp"),
        "expected tool.send_otp select: {v}"
    );
    assert!(
        !step_named(&v, "ProposePath"),
        "must not cold ProposePath when tool harness installed: {v}"
    );
    assert_eq!(v["llm_calls"], 0);
    let text = v["reply"]["text"].as_str().unwrap_or("");
    assert!(text.contains("OTP sent"), "reply={text}");
}

#[tokio::test]
async fn confirm_then_act_asks_without_yes() {
    let app = app("confirm");
    let (status, v) = post_turn(&app, "t-c1", "please confirm").await;
    assert_eq!(status, StatusCode::OK, "{v}");
    // "please confirm" alone may not match intent — if it does, should ask.
    if step_named(&v, "Harness.Tool") {
        let text = v["reply"]["text"].as_str().unwrap_or("");
        assert!(
            text.to_lowercase().contains("confirm"),
            "expected confirm prompt, got {text}"
        );
        assert!(!step_named(&v, "ProposePath"));
    }
}

#[tokio::test]
async fn confirm_and_proceed_runs_tool() {
    let app = app("confirm2");
    let (status, v) = post_turn(&app, "t-c2", "yes confirm and proceed").await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(
        step_detail_contains(&v, "Conductor.Select", "workflow.confirm_then_act"),
        "expected confirm_then_act: {v}"
    );
    let text = v["reply"]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("Done") || text.contains("executed"),
        "reply={text}"
    );
    assert!(!step_named(&v, "ProposePath"));
}

#[tokio::test]
async fn please_confirm_asks_without_executing() {
    let app = app("confirm3");
    let (status, v) = post_turn(&app, "t-c3", "please confirm").await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(
        step_detail_contains(&v, "Conductor.Select", "workflow.confirm_then_act"),
        "{v}"
    );
    let text = v["reply"]["text"].as_str().unwrap_or("").to_lowercase();
    assert!(text.contains("confirm"), "reply={text}");
    assert!(!step_named(&v, "ProposePath"));
}

#[tokio::test]
async fn help_me_uses_sol_understand_intent() {
    let app = app("intent");
    let (status, v) = post_turn(&app, "t-intent", "help me figure out what to do next").await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(
        step_detail_contains(&v, "Conductor.Select", "understand_intent"),
        "expected Sol understand_intent cutover: {v}"
    );
    assert!(!step_named(&v, "ProposePath"));
    let text = v["reply"]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("Intent label=") || text.to_lowercase().contains("clarif"),
        "reply={text}"
    );
}

#[tokio::test]
async fn free_form_uses_full_reply_not_propose_path() {
    let app = app("full");
    let (status, v) = post_turn(&app, "t-full", "tell me something interesting about cats").await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(
        step_detail_contains(&v, "Conductor.Select", "full_reply"),
        "expected full_reply catch-all: {v}"
    );
    assert!(!step_named(&v, "ProposePath"));
    assert_eq!(v["llm_calls"], 0);
    let text = v["reply"]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("fuller answer") || text.contains("cats"),
        "reply={text}"
    );
}
