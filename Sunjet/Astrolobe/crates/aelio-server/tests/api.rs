use aelio::runtime::{DurableRuntime, World};
use aelio::storage::AelioStore;
use aelio_server::{router, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use ll_query::Database;
use serde_json::{json, Value};
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
