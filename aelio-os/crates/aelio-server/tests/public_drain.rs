#![cfg(unix)]

use std::net::TcpListener as StdTcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::{Json, Router};
use serde_json::{json, Value};

#[derive(Clone)]
struct SlowHost {
    calls: Arc<AtomicUsize>,
}

async fn slow_read(State(state): State<SlowHost>) -> Json<Value> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    Json(json!({"outcome":"ok","output":{"value":"stable"},"usage_tokens":0}))
}

fn unused_address() -> String {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    drop(listener);
    address
}

fn start_server(data_dir: &std::path::Path, bind: &str, host_url: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_aelio-server"))
        .env("AELIO_ALLOW_INSECURE_OPEN", "1")
        .env("AELIO_RUNTIME_BIND", bind)
        .env("AELIO_DATA_DIR", data_dir)
        .env("AELIO_TENANT_ID", "tenant-a")
        .env("AELIO_HOST_URL", host_url)
        .env("AELIO_HOST_TOKEN", "drain-host-token")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

async fn wait_ready(client: &reqwest::Client, base: &str) {
    for _ in 0..100 {
        if client
            .get(format!("{base}/readyz"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not become ready");
}

async fn post(client: &reqwest::Client, base: &str, path: &str, body: Value) -> Value {
    let response = client
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert!(status.is_success(), "{status}: {body}");
    body
}

fn terminate(child: &Child) {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
}

async fn wait_exit(child: &mut Child) {
    for _ in 0..100 {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = child.kill();
    panic!("server did not finish graceful drain within five seconds");
}

fn gate_cases() -> Vec<Value> {
    (0..20)
        .map(|index| {
            json!({
                "input":{"turn":{"index":index}},"wakes":[],"expect_park":false,
                "expected":{"turn":{"index":index},"read":{"value":"stable"}},
                "fixtures":[{"target":"slow.read@1","output":{"value":"stable"},"usage_tokens":0}]
            })
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigterm_drains_queued_turns_and_restart_recovers_active_build() {
    let root = tempfile::tempdir().unwrap();
    let host_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host_address = host_listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let host_app = Router::new()
        .route("/internal/aelio/target", axum::routing::post(slow_read))
        .with_state(SlowHost {
            calls: Arc::clone(&calls),
        });
    let host_task = tokio::spawn(async move {
        axum::serve(host_listener, host_app).await.unwrap();
    });

    let bind = unused_address();
    let base = format!("http://{bind}");
    let host_url = format!("http://{host_address}");
    let mut server = start_server(root.path(), &bind, &host_url);
    let client = reqwest::Client::new();
    wait_ready(&client, &base).await;

    let flow = json!({
        "tenant":"tenant-a","flow_id":"flow.slow-read","flow_rev":"1",
        "program":{
            "nid":"read","op":"Call","id":"slow.read@1",
            "args":{"turn":{"pull":"turn"}},"into":"read"
        },
        "targets":[{
            "id":"slow.read@1","class":"tool","effect":"read",
            "input_imprint":"aelio.turn.input@1","output_imprint":"aelio.turn.output@1",
            "bounded":{"kind":"deadline","max_ms":5000},"policy_tags":[],"origin":"tenant"
        }],"prompts":[]
    });
    post(&client, &base, "/v1/flows", flow.clone()).await;
    post(
        &client,
        &base,
        "/v1/flows/gate",
        json!({"flow":flow,"cases":gate_cases()}),
    )
    .await;

    let examples: Vec<_> = (0..3)
        .map(|index| {
            json!({
                "inputs":{"turn":{"index":index}},"output":{"ok":true},
                "negative":index == 0,"fixtures":[]
            })
        })
        .collect();
    let build = post(
        &client,
        &base,
        "/v1/builds",
        json!({"spec":{
            "name":"flow.drain-build","description":"durable active build",
            "inputs":[{"name":"turn","imprint":"aelio.turn.input@1","required":true,"sensitivity":"internal"}],
            "output":"aelio.turn.output@1",
            "budget":{"max_depth":4,"max_children":4,"max_llm_calls":4,"max_tokens":10000,"max_reactions":100,"max_wall_ms":60000},
            "scope":{"tenant":"tenant-a","registries":["flow.*"]},
            "policy":{"principal_grants":[],"allowed_effects":["pure"],"denied_effects":["write","external"]},
            "examples":examples
        }}),
    )
    .await;
    let build_id = build["build_id"].as_str().unwrap().to_owned();
    let advanced = post(
        &client,
        &base,
        &format!("/v1/builds/{build_id}/advance"),
        json!({"tenant":"tenant-a"}),
    )
    .await;
    assert_eq!(advanced["action"], "progress");
    assert_eq!(advanced["job"]["stage"], "resolve");

    let turn = json!({
        "tenant":"tenant-a","instance_id":"drain-instance",
        "flow_id":"flow.slow-read","flow_rev":"1","input":{"turn":{"query":"same"}}
    });
    let first_client = client.clone();
    let first_base = base.clone();
    let first_turn = turn.clone();
    let first =
        tokio::spawn(
            async move { post(&first_client, &first_base, "/v1/turns", first_turn).await },
        );
    for _ in 0..50 {
        if calls.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let second_client = client.clone();
    let second_base = base.clone();
    let second =
        tokio::spawn(async move { post(&second_client, &second_base, "/v1/turns", turn).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    terminate(&server);

    let first = first.await.unwrap();
    let second = second.await.unwrap();
    assert_eq!(first, second);
    assert_eq!(first["bag"]["read"]["value"], "stable");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    wait_exit(&mut server).await;

    let mut restarted = start_server(root.path(), &bind, &host_url);
    wait_ready(&client, &base).await;
    let replay = post(
        &client,
        &base,
        "/v1/turns",
        json!({
            "tenant":"tenant-a","instance_id":"drain-instance",
            "flow_id":"flow.slow-read","flow_rev":"1","input":{"turn":{"query":"same"}}
        }),
    )
    .await;
    assert_eq!(replay, first);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let recovered: Value = client
        .get(format!("{base}/v1/builds/{build_id}?tenant=tenant-a"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered["stage"], "resolve");
    let resumed = post(
        &client,
        &base,
        &format!("/v1/builds/{build_id}/advance"),
        json!({"tenant":"tenant-a"}),
    )
    .await;
    assert_eq!(resumed["action"], "progress");
    assert_eq!(resumed["job"]["stage"], "search");

    terminate(&restarted);
    wait_exit(&mut restarted).await;
    host_task.abort();
}
