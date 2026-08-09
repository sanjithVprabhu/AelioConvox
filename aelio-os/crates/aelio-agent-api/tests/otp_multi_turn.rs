//! Multi-turn OTP: send → wait for code → verify (session table + tool harness).

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
    let path = std::env::temp_dir().join(format!("aelio_otp_{tag}_{}", std::process::id()));
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
                        "user_id": "u-otp",
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

#[tokio::test]
async fn send_otp_then_code_verifies_login() {
    let app = app("flow");
    let (s1, v1) = post_turn(&app, "t1", "send otp to 9611266596").await;
    assert_eq!(s1, StatusCode::OK, "{v1}");
    assert_eq!(v1["suspended"], true);
    let text1 = v1["reply"]["text"].as_str().unwrap_or("");
    assert!(text1.contains("OTP") || text1.contains("code"), "{text1}");
    assert!(!v1["steps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "ProposePath"));

    let (s2, v2) = post_turn(&app, "t2", "123456").await;
    assert_eq!(s2, StatusCode::OK, "{v2}");
    let text2 = v2["reply"]["text"].as_str().unwrap_or("");
    assert!(
        text2.to_lowercase().contains("verified") || text2.to_lowercase().contains("login"),
        "reply={text2}"
    );
    assert_eq!(v2["new_state"], "authenticated");
}

#[tokio::test]
async fn bad_otp_code_fails_closed() {
    let app = app("bad");
    let _ = post_turn(&app, "t1", "send otp to 9611266596").await;
    let (s2, v2) = post_turn(&app, "t2", "000000").await;
    assert_eq!(s2, StatusCode::OK, "{v2}");
    let text2 = v2["reply"]["text"].as_str().unwrap_or("").to_lowercase();
    assert!(text2.contains("invalid"), "{text2}");
}
