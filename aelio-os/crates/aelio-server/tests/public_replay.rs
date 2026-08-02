use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aelio_db_query::{ColumnKind, Database, Value as DbValue};
use aelio_runtime::{Runtime, RuntimeConfig, DEFAULT_QUEUE_DEPTH};
use aelio_server::ServerState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "aelio-public-replay-token";

#[derive(Clone)]
struct ReadHost {
    database: Arc<Mutex<Database>>,
    row_id: u64,
    calls: Arc<AtomicUsize>,
}

async fn target_host(State(state): State<ReadHost>, Json(request): Json<Value>) -> Json<Value> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    assert_eq!(request["target"], "database.read@1");
    let value = state
        .database
        .lock()
        .unwrap()
        .get_row_values("source", state.row_id)
        .unwrap()
        .unwrap()
        .into_iter()
        .find_map(|(name, value)| {
            (name == "value")
                .then_some(value)
                .and_then(|value| match value {
                    DbValue::Utf8(value) => Some(value),
                    _ => None,
                })
        })
        .unwrap();
    Json(json!({"outcome":"ok","output":{"value":value},"usage_tokens":0}))
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn historical_replay_injects_ledgered_read_after_database_change() {
    let root = tempfile::tempdir().unwrap();
    let database_dir = root.path().join("source-db");
    std::fs::create_dir_all(&database_dir).unwrap();
    let mut database = Database::create(database_dir).unwrap();
    database
        .create_table("source", &[("value", ColumnKind::Utf8)])
        .unwrap();
    let row_id = database
        .insert("source", &[("value", DbValue::Utf8("old-value".into()))])
        .unwrap();
    let database = Arc::new(Mutex::new(database));
    let calls = Arc::new(AtomicUsize::new(0));
    let host = ReadHost {
        database: Arc::clone(&database),
        row_id,
        calls: Arc::clone(&calls),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let host_task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/internal/aelio/target", axum::routing::post(target_host))
                .with_state(host),
        )
        .await
        .unwrap();
    });

    let runtime_dir = root.path().join("runtime");
    let runtime = tokio::task::spawn_blocking(move || {
        Runtime::open(RuntimeConfig {
            data_dir: runtime_dir,
            host_token: Some("local-read-host-token".into()),
            host_url: Some(format!("http://{address}")),
            event_key_secret: [92; 32],
            queue_depth: DEFAULT_QUEUE_DEPTH,
        })
        .unwrap()
    })
    .await
    .unwrap();
    let app = aelio_server::router(
        ServerState::new_tenant_scoped(runtime, vec![("tenant-a".into(), TOKEN.into())]).unwrap(),
    );
    let flow = json!({
        "tenant":"tenant-a","flow_id":"flow.historical-read","flow_rev":"1",
        "program":{
            "nid":"read","op":"Call","id":"database.read@1",
            "args":{"turn":{"pull":"turn"}},"into":"read"
        },
        "targets":[{
            "id":"database.read@1","class":"tool","effect":"read",
            "input_imprint":"aelio.turn.input@1","output_imprint":"aelio.turn.output@1",
            "bounded":{"kind":"deadline","max_ms":5000},"policy_tags":[],"origin":"tenant"
        }],
        "prompts":[]
    });
    let (status, pushed) = post(&app, "/v1/flows", flow.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{pushed}");
    let cases: Vec<_> = (0..20)
        .map(|index| {
            let input = json!({"turn":{"query":index}});
            json!({
                "input":input,"wakes":[],"expect_park":false,
                "expected":{"turn":{"query":index},"read":{"value":"old-value"}},
                "fixtures":[{"target":"database.read@1","output":{"value":"old-value"},"usage_tokens":0}]
            })
        })
        .collect();
    let (status, gated) = post(&app, "/v1/flows/gate", json!({"flow":flow,"cases":cases})).await;
    assert_eq!(status, StatusCode::OK, "{gated}");
    assert_eq!(gated["record"]["status"], "canary");

    let initial_input = json!({"turn":{"query":"historical"}});
    let (status, live) = post(
        &app,
        "/v1/turns",
        json!({
            "tenant":"tenant-a","instance_id":"historical-read-1",
            "flow_id":"flow.historical-read","flow_rev":"1","input":initial_input
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{live}");
    assert_eq!(live["bag"]["read"]["value"], "old-value");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    database
        .lock()
        .unwrap()
        .update(
            "source",
            row_id,
            &[("value", DbValue::Utf8("new-value".into()))],
        )
        .unwrap();
    let (status, replay) = post(
        &app,
        "/v1/replay",
        json!({
            "tenant":"tenant-a","instance_id":"historical-read-1",
            "flow_id":"flow.historical-read","flow_rev":"1",
            "initial_input":{"turn":{"query":"historical"}}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["matched"], true);
    assert_eq!(replay["live_bag_hash"], live["bag_hash"]);
    assert_eq!(replay["replay_bag_hash"], live["bag_hash"]);
    assert!(replay["ledger_entries"].as_u64().unwrap() >= 3);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "replay must not execute the historical database read"
    );

    let (status, fresh) = post(
        &app,
        "/v1/turns",
        json!({
            "tenant":"tenant-a","instance_id":"historical-read-2",
            "flow_id":"flow.historical-read","flow_rev":"1",
            "input":{"turn":{"query":"fresh"}}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    assert_eq!(fresh["bag"]["read"]["value"], "new-value");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    host_task.abort();
    // `reqwest::blocking::Client` owns an internal runtime whose final drop must not occur inside
    // this async test context. Production drops it from the synchronous process boundary.
    tokio::task::spawn_blocking(move || drop(app))
        .await
        .unwrap();
}
