//! SunJet daemon — outbox push channel + ll-server maintenance.
//!
//! Environment:
//! - `SUNJET_DAEMON_BIND` — listen address (default `127.0.0.1:8081`)
//! - `SUNJET_DAEMON_API_KEYS` — comma-separated bearer keys (open mode if unset)
//! - `SUNJET_LLSERVER_URL` — ll-server base URL for compaction (optional)
//! - `SUNJET_LLSERVER_API_KEY` — bearer key for ll-server admin endpoints
//! - `SUNJET_COMPACT_INTERVAL_S` — compaction interval (default 300)

use std::collections::{HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Clone)]
struct AppState {
    keys: Arc<KeySet>,
    queue: Arc<Mutex<VecDeque<OutboxJob>>>,
    http: reqwest::Client,
    llserver_url: Option<String>,
    llserver_key: Option<String>,
}

enum KeySet {
    Open,
    Keys(HashSet<String>),
}

impl KeySet {
    fn accepts(&self, token: &str) -> bool {
        match self {
            KeySet::Open => true,
            KeySet::Keys(s) => s.contains(token),
        }
    }
    fn is_open(&self) -> bool {
        matches!(self, KeySet::Open)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct OutboxJob {
    job_id: String,
    tenant_id: String,
    #[serde(default)]
    identity_id: Option<String>,
    channel: String,
    event_type: String,
    payload: serde_json::Value,
    #[serde(default)]
    webhook_url: Option<String>,
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    service: &'static str,
    queued: usize,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let bind = std::env::var("SUNJET_DAEMON_BIND").unwrap_or_else(|_| "127.0.0.1:8081".to_string());
    let api_keys: Vec<String> = std::env::var("SUNJET_DAEMON_API_KEYS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let keys = if api_keys.is_empty() {
        eprintln!("WARNING: SUNJET_DAEMON_API_KEYS unset — open mode");
        KeySet::Open
    } else {
        KeySet::Keys(api_keys.into_iter().collect())
    };

    let state = AppState {
        keys: Arc::new(keys),
        queue: Arc::new(Mutex::new(VecDeque::new())),
        http: reqwest::Client::new(),
        llserver_url: std::env::var("SUNJET_LLSERVER_URL").ok(),
        llserver_key: std::env::var("SUNJET_LLSERVER_API_KEY").ok(),
    };

    spawn_delivery_worker(state.clone());
    spawn_compact_worker(state.clone());
    spawn_sweep_worker(state.clone());
    spawn_rollup_worker(state.clone());

    let protected = Router::new()
        .route("/v1/outbox/enqueue", post(enqueue))
        .route("/v1/outbox/pending", get(pending_count))
        .route("/v1/admin/sweep", post(admin_sweep))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .with_state(state.clone());

    let app = Router::new()
        .route("/v1/health", get(health))
        .merge(protected)
        .with_state(state);

    let addr: SocketAddr = bind.parse().expect("invalid SUNJET_DAEMON_BIND");
    eprintln!("sunjet-daemon listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn auth(State(state): State<AppState>, req: Request, next: Next) -> Result<Response, StatusCode> {
    if state.keys.is_open() {
        return Ok(next.run(req).await);
    }
    let presented = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    match presented {
        Some(token) if state.keys.as_ref().accepts(token) => Ok(next.run(req).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    let queued = state.queue.lock().await.len();
    Json(Health {
        status: "ok",
        service: "sunjet-daemon",
        queued,
    })
}

async fn pending_count(State(state): State<AppState>) -> Json<serde_json::Value> {
    let queued = state.queue.lock().await.len();
    Json(serde_json::json!({ "queued": queued }))
}

async fn enqueue(State(state): State<AppState>, Json(job): Json<OutboxJob>) -> Json<serde_json::Value> {
    state.queue.lock().await.push_back(job);
    Json(serde_json::json!({ "queued": true }))
}

fn spawn_delivery_worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let job = { state.queue.lock().await.pop_front() };
            let Some(job) = job else { continue };
            if job.channel == "webhook" {
                if let Some(url) = job.webhook_url.as_deref() {
                    let body = serde_json::json!({
                        "jobId": job.job_id,
                        "tenantId": job.tenant_id,
                        "identityId": job.identity_id,
                        "eventType": job.event_type,
                        "payload": job.payload,
                    });
                    match state.http.post(url).json(&body).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            tracing::info!(job_id = %job.job_id, "webhook delivered");
                        }
                        Ok(resp) => {
                            tracing::warn!(job_id = %job.job_id, status = %resp.status(), "webhook failed");
                            state.queue.lock().await.push_back(job);
                        }
                        Err(err) => {
                            tracing::warn!(job_id = %job.job_id, %err, "webhook error");
                            state.queue.lock().await.push_back(job);
                        }
                    }
                }
            } else {
                tracing::info!(
                    job_id = %job.job_id,
                    event = %job.event_type,
                    tenant = %job.tenant_id,
                    "convox_ws job recorded (delivery via Aelio outbox worker)"
                );
            }
        }
    });
}

async fn admin_sweep(State(state): State<AppState>) -> Json<serde_json::Value> {
    let deleted = sweep_expired_runtime_state(&state).await;
    Json(serde_json::json!({ "deleted": deleted }))
}

async fn sweep_expired_runtime_state(state: &AppState) -> usize {
    let Some(base) = state.llserver_url.as_deref() else {
        return 0;
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let scan_body = serde_json::json!({
        "k": 500,
        "filters": [
            { "col": "expires_at", "op": "lt", "value": { "type": "i64", "value": now } }
        ]
    });

    let mut scan_req = state
        .http
        .post(format!("{base}/v1/tables/runtime_state/scan"))
        .json(&scan_body);
    if let Some(key) = state.llserver_key.as_deref() {
        scan_req = scan_req.header(AUTHORIZATION, format!("Bearer {key}"));
    }

    let rows: Vec<serde_json::Value> = match scan_req.send().await {
        Ok(resp) if resp.status().is_success() => resp
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v.get("rows").and_then(|r| r.as_array()).cloned())
            .unwrap_or_default(),
        Ok(resp) => {
            tracing::warn!(status = %resp.status(), "runtime_state scan failed");
            return 0;
        }
        Err(err) => {
            tracing::warn!(%err, "runtime_state scan error");
            return 0;
        }
    };

    let mut deleted = 0usize;
    for row in rows {
        let Some(row_id) = row.get("row_id").and_then(|v| v.as_u64()) else {
            continue;
        };
        let mut del_req = state
            .http
            .delete(format!("{base}/v1/tables/runtime_state/rows/{row_id}"));
        if let Some(key) = state.llserver_key.as_deref() {
            del_req = del_req.header(AUTHORIZATION, format!("Bearer {key}"));
        }
        match del_req.send().await {
            Ok(resp) if resp.status().is_success() => deleted += 1,
            Ok(resp) => tracing::warn!(row_id, status = %resp.status(), "runtime_state delete failed"),
            Err(err) => tracing::warn!(row_id, %err, "runtime_state delete error"),
        }
    }
    deleted
}

fn spawn_sweep_worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let deleted = sweep_expired_runtime_state(&state).await;
            if deleted > 0 {
                tracing::info!(deleted, "runtime_state sweep ok");
            }
        }
    });
}

fn spawn_rollup_worker(state: AppState) {
    let aelio_url = std::env::var("AELIO_API_URL").ok();
    let daemon_key = std::env::var("SUNJET_DAEMON_API_KEYS")
        .ok()
        .and_then(|s| s.split(',').next().map(|k| k.trim().to_string()));
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(600)).await;
            let Some(base) = aelio_url.as_deref() else { continue };
            let mut req = state.http.post(format!("{base}/api/v1/internal/memory/rollup"));
            if let Some(key) = daemon_key.as_deref() {
                req = req.header(AUTHORIZATION, format!("Bearer {key}"));
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => tracing::info!("memory rollup ok"),
                Ok(resp) => tracing::warn!(status = %resp.status(), "memory rollup failed"),
                Err(err) => tracing::warn!(%err, "memory rollup error"),
            }
        }
    });
}

fn spawn_compact_worker(state: AppState) {
    let interval_s: u64 = std::env::var("SUNJET_COMPACT_INTERVAL_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(interval_s)).await;
            let Some(base) = state.llserver_url.as_deref() else { continue };
            let mut req = state.http.post(format!("{base}/v1/admin/compact"));
            if let Some(key) = state.llserver_key.as_deref() {
                req = req.header(AUTHORIZATION, format!("Bearer {key}"));
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => tracing::info!("ll-server compact ok"),
                Ok(resp) => tracing::warn!(status = %resp.status(), "ll-server compact failed"),
                Err(err) => tracing::warn!(%err, "ll-server compact error"),
            }
        }
    });
}