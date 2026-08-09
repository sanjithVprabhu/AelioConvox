//! Rust HTTP boundary for the durable Aelio runtime.

pub mod adaptive_bridge;
mod agent_loop;
mod agent_loop_store;
mod flow_lowering;
mod harness_mode;
mod runtime_adaptive_host;
mod runtime_tool_host;
pub mod sdk_bridge;

pub use harness_mode::{
    log_harness_mode_banner, resolve_harness_mode, resolve_legacy_spine, HarnessMode,
};

use std::collections::HashMap;
use std::sync::Arc;

use aelio_agent::runtime::durable::validate_catalog;
use aelio_agent::runtime::{
    DurableRuntime, DurableScheduler, DurableTurnRequest, ExplorationPolicy, ProactiveCandidate,
    ProactiveDecision, ProactiveLoop, ProactivePolicy, SagaReconciler, ScheduledJob,
};
use aelio_agent::storage::PutIfAbsent;
use aelio_agent::tenant::TenantDecl;
use aelio_agent::{AelioError, ReasonCode};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use sdk_bridge::{BridgeConfig, ResultMessage, SdkBridge};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    runtime: Arc<Mutex<DurableRuntime>>,
    credentials: Arc<HashMap<String, Credential>>,
    bridge: SdkBridge,
    requires_sdk_bridge: bool,
    artifact_runtime: Option<aelio_runtime::Runtime>,
    /// Conductor/Harness OS store (events, process tree, shadow).
    /// Defaults to MemoryStore; set `AELIO_OS_STORE_PATH` for EmbeddedStore (durable).
    os_store: Arc<Mutex<Box<dyn aelio_store::Store + Send>>>,
}

#[derive(Clone)]
struct AuthenticatedTenant {
    tenant_id: String,
    principal: String,
}

#[derive(Clone)]
struct Credential {
    tenant_id: String,
    principal: String,
    admin: bool,
}

#[derive(Debug, Deserialize)]
struct RegisterMessage {
    #[serde(rename = "type")]
    message_type: String,
    catalog: TenantDecl,
}

impl AppState {
    pub fn new(runtime: DurableRuntime, api_keys: Vec<String>) -> Self {
        Self::build(
            runtime,
            api_keys.clone(),
            api_keys,
            BridgeConfig::default(),
            false,
            None,
            false,
        )
    }

    /// Unified production constructor: the adaptive agent selects capabilities, but every tool
    /// effect executes through a gated `aelio-runtime` proxy artifact. Legacy FlowSpec execution
    /// is always disabled.
    pub fn new_with_artifact_runtime(
        runtime: DurableRuntime,
        api_keys: Vec<String>,
        artifact_runtime: aelio_runtime::Runtime,
    ) -> Self {
        Self::build(
            runtime,
            api_keys.clone(),
            api_keys,
            BridgeConfig::default(),
            false,
            Some(artifact_runtime),
            false,
        )
    }

    /// Parity/test constructor: installs the SDK-backed capability host used by wire-level
    /// scenarios without an artifact runtime. Legacy FlowSpec execution remains disabled; do not
    /// use this as a production authority path.
    pub fn new_with_sdk_bridge(
        runtime: DurableRuntime,
        api_keys: Vec<String>,
        bridge_config: BridgeConfig,
    ) -> Self {
        Self::build(
            runtime,
            api_keys.clone(),
            api_keys,
            bridge_config,
            true,
            None,
            false,
        )
    }

    /// Parity/test constructor with distinct runtime/SDK and administrative credentials.
    /// Legacy FlowSpec execution remains disabled.
    pub fn new_with_scoped_sdk_bridge(
        runtime: DurableRuntime,
        api_keys: Vec<String>,
        admin_api_keys: Vec<String>,
        bridge_config: BridgeConfig,
    ) -> Self {
        Self::build(
            runtime,
            api_keys,
            admin_api_keys,
            bridge_config,
            true,
            None,
            false,
        )
    }

    /// Local/parity SDK wire fixtures that still drive the demo FlowSpec interpreter.
    /// Production never calls this; prefer `new_with_artifact_runtime` + lowered flows.
    pub fn new_with_scoped_sdk_bridge_for_legacy_parity(
        runtime: DurableRuntime,
        api_keys: Vec<String>,
        admin_api_keys: Vec<String>,
        bridge_config: BridgeConfig,
    ) -> Self {
        Self::build(
            runtime,
            api_keys,
            admin_api_keys,
            bridge_config,
            true,
            None,
            true,
        )
    }

    /// Local/parity HTTP fixtures only. Same as [`Self::new`] but re-enables the legacy FlowSpec
    /// interpreter after the fail-closed default. Production never calls this.
    pub fn new_for_legacy_parity(runtime: DurableRuntime, api_keys: Vec<String>) -> Self {
        Self::build(
            runtime,
            api_keys.clone(),
            api_keys,
            BridgeConfig::default(),
            false,
            None,
            true,
        )
    }

    fn build(
        mut runtime: DurableRuntime,
        api_keys: Vec<String>,
        admin_api_keys: Vec<String>,
        bridge_config: BridgeConfig,
        install_host: bool,
        artifact_runtime: Option<aelio_runtime::Runtime>,
        legacy_parity: bool,
    ) -> Self {
        let tenant_id = runtime.world.tenant.tenant_id.clone();
        let mut credentials = HashMap::new();
        for (admin, keys) in [(false, api_keys), (true, admin_api_keys)] {
            for key in keys {
                let digest = Sha256::digest(key.as_bytes());
                credentials.insert(
                    key,
                    Credential {
                        tenant_id: tenant_id.clone(),
                        principal: format!("api-key:{}", hex::encode(&digest[..8])),
                        admin,
                    },
                );
            }
        }
        // Every public AppState constructor refuses the legacy FlowSpec interpreter unless an
        // explicit local/parity constructor opts back in after the fail-closed default.
        runtime.world.disable_legacy_flow_execution();
        if legacy_parity {
            runtime.world.enable_legacy_flow_execution_for_parity();
        }
        let bridge = SdkBridge::new(bridge_config);
        if let Some(artifact_runtime) = &artifact_runtime {
            runtime.install_tool_host(Box::new(runtime_tool_host::RuntimeArtifactToolHost::new(
                artifact_runtime.clone(),
                tenant_id.clone(),
            )));
            runtime.world.set_adaptive_artifact_host(Box::new(
                runtime_adaptive_host::RuntimeAdaptiveArtifactHost::new(artifact_runtime.clone()),
            ));
        } else if install_host {
            runtime.install_tool_host(Box::new(bridge.tool_host(tenant_id)));
        }
        Self {
            runtime: Arc::new(Mutex::new(runtime)),
            credentials: Arc::new(credentials),
            bridge,
            requires_sdk_bridge: install_host && artifact_runtime.is_none(),
            artifact_runtime,
            os_store: Arc::new(Mutex::new(open_os_store())),
        }
    }

    pub fn is_open(&self) -> bool {
        self.credentials.is_empty()
    }

    pub fn bridge(&self) -> SdkBridge {
        self.bridge.clone()
    }
}

/// Open the Conductor/Harness OS store.
///
/// - `AELIO_OS_STORE_PATH` set → durable [`aelio_store::EmbeddedStore`]
/// - otherwise → in-memory (tests / dev)
///
/// Always attempts vendor library install into `tenant` at first use via
/// [`ensure_vendor_library`].
fn open_os_store() -> Box<dyn aelio_store::Store + Send> {
    match std::env::var("AELIO_OS_STORE_PATH") {
        Ok(path) if !path.trim().is_empty() => match aelio_store::EmbeddedStore::open(&path) {
            Ok(store) => {
                eprintln!("aelio-agent-api: OS store durable at {path}");
                Box::new(store)
            }
            Err(err) => {
                eprintln!(
                        "aelio-agent-api: failed to open AELIO_OS_STORE_PATH={path} ({err:?}); falling back to memory"
                    );
                Box::new(aelio_store::MemoryStore::new())
            }
        },
        _ => Box::new(aelio_store::MemoryStore::new()),
    }
}

/// Idempotent install of on-disk vendor library into the OS store for `tenant`.
fn ensure_vendor_library(store: &mut dyn aelio_store::Store, tenant: &str) {
    let Some(root) = aelio_kernel::resolve_library_root(None) else {
        return;
    };
    match aelio_kernel::install_from_manifest(store, tenant, &root) {
        Ok(report) => {
            let n = report.entries.len();
            if std::env::var("AELIO_OS_LIBRARY_LOG").is_ok() {
                eprintln!("aelio-agent-api: vendor library entries={n} tenant={tenant}");
            }
        }
        Err(err) => {
            eprintln!("aelio-agent-api: vendor library install skipped: {err:?}");
        }
    }
}

fn tool_harness_turn_result(
    turn_id: &str,
    harness_id: &str,
    bag: aelio_sol::SolValue,
    bag_hash: &str,
) -> aelio_agent::blocks::turn::TurnResult {
    let text = aelio_kernel::tool_bag_reply_text(harness_id, &bag);
    let mut reply = aelio_agent::abilities::express::Utterance::plain(
        text,
        aelio_agent::abilities::express::ExpressVia::Template,
    );
    reply.template_id = Some(format!("harness.{harness_id}"));
    let mut result = aelio_agent::blocks::turn::TurnResult {
        reply,
        llm_calls: 0,
        tier: None,
        depth: aelio_agent::Depth::Deep,
        steps: vec![
            aelio_agent::blocks::turn::TurnTraceStep {
                name: "Conductor.Select".into(),
                detail: format!("harness={harness_id} — installed tool/memory program"),
            },
            aelio_agent::blocks::turn::TurnTraceStep {
                name: "Harness.Load".into(),
                detail: format!("id={harness_id} hash={bag_hash}"),
            },
            aelio_agent::blocks::turn::TurnTraceStep {
                name: "Harness.Tool".into(),
                detail: format!("executed Sol program id={harness_id}"),
            },
            aelio_agent::blocks::turn::TurnTraceStep {
                name: "Conductor.Shadow".into(),
                detail: format!(
                    "agent=escalate os={harness_id} agree=true comparable=true propose=false tool_cutover=true"
                ),
            },
        ],
        opened_loop: false,
        new_state: None,
        active_flow: None,
        situation_hash: None,
        proposal_id: None,
        suspended: bag_hash.starts_with("parked:"),
        graph_suspension: None,
    };
    let _ = result
        .reply
        .ensure_render_frame(turn_id, &format!("rf-{}", result.steps.len()));
    result
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/turns", post(process_turn))
        // Conductor/Harness OS event admission (plan §10 / Phase 4.2–4.3).
        .route("/v2/events", post(process_event_v2))
        .route("/v1/users/state", post(set_user_state))
        .route("/v1/catalog", get(active_catalog).post(register_catalog))
        .route("/v1/sdk", get(sdk_socket))
        .route("/v1/admin/flush", post(flush))
        .route("/v1/admin/jobs", post(schedule_job))
        .route("/v1/proactive/evaluate", post(evaluate_proactive))
        .route("/v1/admin/workers/tick", post(worker_tick))
        .route("/v1/admin/workers/complete", post(worker_complete))
        .route("/v1/admin/workers/fail", post(worker_fail))
        .route("/v1/admin/exploration", post(configure_exploration))
        .route("/v1/admin/sdk-delivery", get(list_sdk_delivery_audit))
        .route("/v1/admin/tool-outcomes", get(list_tool_outcomes))
        .route("/v1/admin/open-loops", get(list_open_loops))
        .route("/v1/admin/proposals", get(list_learning_proposals))
        .route(
            "/v1/admin/proposals/{id}/approve",
            post(approve_learning_proposal),
        )
        .route(
            "/v1/admin/proposals/{id}/promote",
            post(promote_learning_proposal),
        )
        .route("/v1/admin/procedures", get(list_learning_procedures))
        .route(
            "/v1/admin/procedures/{id}/suspend",
            post(suspend_learning_procedure),
        )
        .route(
            "/v1/admin/procedures/{id}/bind-artifact",
            post(bind_procedure_artifact),
        )
        .route(
            "/v1/admin/flows/{id}/bind-artifact",
            post(bind_flow_artifact),
        )
        .route(
            "/v1/admin/flow-candidates",
            get(list_flow_candidates).post(propose_flow_candidate),
        )
        .route(
            "/v1/admin/flow-candidates/{key}/review",
            post(review_flow_candidate),
        )
        .route(
            "/v1/admin/flow-candidates/{key}/activate",
            post(activate_flow_candidate),
        )
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state.clone());
    Router::new()
        .route("/v1/health", get(health))
        .with_state(state)
        .merge(protected)
        // Bound JSON and other HTTP request bodies before deserialization. The SDK WebSocket has
        // its own 1 MiB message bound at upgrade time.
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let runtime = state.runtime.lock().await;
    let tenant_id = runtime.world.tenant.tenant_id.clone();
    let embedding_space_id = runtime.embedding_space_id();
    let semantic_embedder = runtime.has_semantic_embedder();
    let catalog_registered =
        !runtime.world.tenant.states.is_empty() && !runtime.world.tenant.personalities.is_empty();
    let install = catalog_install_status(&runtime, state.artifact_runtime.is_some());
    let sdk_required = state.requires_sdk_bridge && !runtime.world.tenant.tools.is_empty();
    let sdk = state.bridge.catalog_status(&tenant_id);
    let sdk_registered = sdk.is_some();
    let sdk_available = sdk.is_some_and(|status| status.available);
    let ready =
        catalog_registered && install.executable_pending == 0 && (!sdk_required || sdk_available);
    Json(serde_json::json!({
        "status": if ready { "ok" } else { "degraded" },
        "ready": ready,
        "service": "aelio-server",
        "version": env!("CARGO_PKG_VERSION"),
        "runtime": "rust",
        "tenant_id": tenant_id,
        "semantic_embedder": semantic_embedder,
        "embedding_space_id": embedding_space_id,
        "authentication_configured": !state.is_open(),
        "catalog_registered": catalog_registered,
        // Install debt: only flows that declared a lowering must pin to be ready.
        "unmaterialized_flows": install.executable_pending,
        "executable_flows": install.executable_flows,
        "semantic_only_flows": install.semantic_only_flows,
        "sdk_required": sdk_required,
        "sdk_registered": sdk_registered,
        "sdk_available": sdk_available,
    }))
}

#[derive(Debug, Clone, Copy)]
struct CatalogInstallStatus {
    executable_flows: usize,
    executable_pending: usize,
    semantic_only_flows: usize,
}

/// OS-install view of the catalog: lowering-declared flows are executable apps that must pin;
/// semantic-only flows are guidance and do not block readiness.
fn catalog_install_status(
    runtime: &DurableRuntime,
    artifact_runtime_enabled: bool,
) -> CatalogInstallStatus {
    let mut executable_flows = 0;
    let mut executable_pending = 0;
    let mut semantic_only_flows = 0;
    for flow in &runtime.world.tenant.flows {
        if flow.lowering.is_some() {
            executable_flows += 1;
            if artifact_runtime_enabled && runtime.world.registry.flow_artifact(&flow.id).is_none()
            {
                executable_pending += 1;
            }
        } else {
            semantic_only_flows += 1;
        }
    }
    CatalogInstallStatus {
        executable_flows,
        executable_pending,
        semantic_only_flows,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnApiRequest {
    turn_id: String,
    user_id: String,
    utterance: String,
    #[serde(default = "unknown_channel")]
    channel: String,
    #[serde(default)]
    memory_subject_id: Option<String>,
}

fn unknown_channel() -> String {
    "unknown".into()
}

/// `POST /v2/events` body — either a full NormalizedEventV1 or a turn adapter envelope.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EventV2Request {
    Normalized(aelio_kernel::NormalizedEventV1),
    TurnAdapter {
        event_id: String,
        user_id: String,
        utterance: String,
        #[serde(default = "unknown_channel")]
        channel: String,
        #[serde(default)]
        source: Option<String>,
        #[serde(default)]
        source_message_id: Option<String>,
        /// When true (default), run deterministic conductor.root after admission.
        #[serde(default = "default_true")]
        run_conductor: bool,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct EventV2Response {
    status: String,
    event_id: String,
    event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    prior_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conductor_route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conductor_decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bag_hash: Option<String>,
    /// True when this path only ran deterministic Conductor (not the full agent turn spine).
    conductor_only: bool,
    /// True when an installed Sol tool/memory/intent harness executed.
    #[serde(default)]
    tool_harness: bool,
}

async fn process_event_v2(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedTenant>,
    Json(request): Json<EventV2Request>,
) -> Result<(StatusCode, Json<EventV2Response>), ApiError> {
    let tenant_id = auth.tenant_id.clone();
    let os_store = Arc::clone(&state.os_store);
    tokio::task::spawn_blocking(move || {
        let (event, run_conductor) = match request {
            EventV2Request::Normalized(mut event) => {
                // Tenant is always derived from auth, never from untrusted model/client override.
                event.tenant_id = tenant_id.clone();
                (event, true)
            }
            EventV2Request::TurnAdapter {
                event_id,
                user_id,
                utterance,
                channel,
                source,
                source_message_id,
                run_conductor,
            } => {
                let event = aelio_kernel::event_from_user_utterance(
                    &tenant_id,
                    &user_id,
                    &channel,
                    &utterance,
                    &event_id,
                    source.as_deref().unwrap_or("api"),
                    source_message_id.as_deref(),
                );
                (event, run_conductor)
            }
        };

        let utterance = event
            .payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut store = os_store.blocking_lock();
        ensure_vendor_library(store.as_mut(), &tenant_id);

        // Greets go through conductor.root (admit_event with run_conductor), not full_reply.
        // Prefer installed tool/intent harnesses for everything else that resolves.
        if !aelio_kernel::should_cutover_deterministic(&utterance) {
            if let Some(intent) = aelio_kernel::resolve_tool_harness_intent(&utterance) {
                if let Ok(Some((bag, hash, harness_id))) =
                    aelio_kernel::try_run_tool_intent(store.as_mut(), &tenant_id, &intent)
                {
                    let admitted = aelio_kernel::admit_event(store.as_mut(), event, false)
                        .map_err(|e| {
                            ApiError(AelioError::new(
                                ReasonCode::Unavailable,
                                format!("event admission failed: {e:?}"),
                            ))
                        })?;
                    let (status, prior) = match &admitted.status {
                        aelio_kernel::AdmitStatus::Accepted => ("accepted".to_string(), None),
                        aelio_kernel::AdmitStatus::Duplicate { prior_event_id } => {
                            ("duplicate".to_string(), Some(prior_event_id.clone()))
                        }
                    };
                    let http = if status == "duplicate" {
                        StatusCode::OK
                    } else {
                        StatusCode::ACCEPTED
                    };
                    let reply = aelio_kernel::tool_bag_reply_text(harness_id, &bag);
                    return Ok((
                        http,
                        Json(EventV2Response {
                            status,
                            event_id: admitted.event.event_id,
                            event_type: admitted.event.event_type,
                            prior_event_id: prior,
                            conductor_route: Some(harness_id.into()),
                            conductor_decision: Some(format!("ToolHarness({harness_id})")),
                            reply_text: Some(reply),
                            bag_hash: Some(hash),
                            conductor_only: true,
                            tool_harness: true,
                        }),
                    ));
                }
            }
        } // end !should_cutover_deterministic tool branch

        let admitted =
            aelio_kernel::admit_event(store.as_mut(), event, run_conductor).map_err(|e| {
                ApiError(AelioError::new(
                    ReasonCode::Unavailable,
                    format!("event admission failed: {e:?}"),
                ))
            })?;

        let (status, prior) = match &admitted.status {
            aelio_kernel::AdmitStatus::Accepted => ("accepted".to_string(), None),
            aelio_kernel::AdmitStatus::Duplicate { prior_event_id } => {
                ("duplicate".to_string(), Some(prior_event_id.clone()))
            }
        };
        let (route, decision, reply, bag_hash) = match admitted.conductor {
            Some(c) => (
                Some(c.route),
                Some(format!("{:?}", c.decision)),
                c.reply_text,
                c.bag_hash,
            ),
            None => (None, None, None, None),
        };
        let http = if status == "duplicate" {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        Ok((
            http,
            Json(EventV2Response {
                status,
                event_id: admitted.event.event_id,
                event_type: admitted.event.event_type,
                prior_event_id: prior,
                conductor_route: route,
                conductor_decision: decision,
                reply_text: reply,
                bag_hash,
                conductor_only: true,
                tool_harness: false,
            }),
        ))
    })
    .await
    .map_err(|e| {
        ApiError(AelioError::new(
            ReasonCode::Internal,
            format!("event worker join failed: {e}"),
        ))
    })?
}

async fn process_turn(
    State(state): State<AppState>,
    Json(request): Json<TurnApiRequest>,
) -> Result<Json<aelio_agent::blocks::turn::TurnResult>, ApiError> {
    if harness_mode::resolve_harness_mode() == HarnessMode::AgentLoop {
        return agent_loop::process_agent_loop_turn(state, request).await;
    }
    let runtime = Arc::clone(&state.runtime);
    let os_store = Arc::clone(&state.os_store);
    tokio::task::spawn_blocking(move || {
        let mut runtime = runtime.blocking_lock();
        if runtime.world.tenant.states.is_empty() || runtime.world.tenant.personalities.is_empty() {
            return Err(ApiError(AelioError::new(
                ReasonCode::Unavailable,
                "tenant catalog is not registered",
            )));
        }
        let install = catalog_install_status(&runtime, state.artifact_runtime.is_some());
        if install.executable_pending > 0 {
            return Err(ApiError(AelioError::new(
                ReasonCode::Unavailable,
                format!(
                    "catalog install incomplete: {} executable flow(s) awaiting materialization",
                    install.executable_pending
                ),
            )));
        }
        let sdk_required = state.requires_sdk_bridge && !runtime.world.tenant.tools.is_empty();
        if sdk_required
            && !state
                .bridge
                .catalog_status(&runtime.world.tenant.tenant_id)
                .is_some_and(|status| status.available)
        {
            return Err(ApiError(AelioError::new(
                ReasonCode::Unavailable,
                "tenant SDK tool host is not connected",
            )));
        }
        // Harness authority (troubleshooting toggle):
        //   AELIO_HARNESS_MODE=agent_loop|legacy|sol|auto
        //   agent_loop branches before this block; auto + artifact_runtime ⇒ legacy.
        let (legacy_spine, harness_mode, harness_mode_reason) =
            harness_mode::resolve_legacy_spine(
                runtime.world.legacy_flow_execution_enabled,
                state.artifact_runtime.is_some(),
            );
        let tenant_id = runtime.world.tenant.tenant_id.clone();
        let turn_id = request.turn_id.clone();
        let user_id = request.user_id.clone();
        let utterance = request.utterance.clone();
        let channel = request.channel.clone();
        let mode_step = aelio_agent::blocks::turn::TurnTraceStep {
            name: "Harness.Mode".into(),
            detail: format!(
                "mode={} legacy_spine={} reason={}",
                harness_mode.as_str(),
                legacy_spine,
                harness_mode_reason
            ),
        };

        // Ensure vendor Sol library is present for tool/memory cutovers.
        {
            let mut store = os_store.blocking_lock();
            ensure_vendor_library(store.as_mut(), &tenant_id);
        }

        // Multi-turn OTP: if user has a pending session and typed a code, complete it.
        {
            let mut store = os_store.blocking_lock();
            if let Ok(Some((ok, msg))) =
                aelio_kernel::try_complete_otp_session(store.as_mut(), &tenant_id, &user_id, &utterance)
            {
                let mut reply = aelio_agent::abilities::express::Utterance::plain(
                    msg,
                    aelio_agent::abilities::express::ExpressVia::Template,
                );
                reply.template_id = Some("tool.otp_login.resume".into());
                let mut result = aelio_agent::blocks::turn::TurnResult {
                    reply,
                    llm_calls: 0,
                    tier: None,
                    depth: aelio_agent::Depth::Deep,
                    steps: vec![
                        mode_step.clone(),
                        aelio_agent::blocks::turn::TurnTraceStep {
                            name: "Harness.Tool".into(),
                            detail: format!(
                                "otp_login resume ok={ok} authority=otp_session"
                            ),
                        },
                        aelio_agent::blocks::turn::TurnTraceStep {
                            name: "Conductor.Select".into(),
                            detail: "harness=tool.otp_login — resume waiting code".into(),
                        },
                    ],
                    opened_loop: false,
                    new_state: if ok {
                        Some("authenticated".into())
                    } else {
                        None
                    },
                    active_flow: None,
                    situation_hash: None,
                    proposal_id: None,
                    suspended: false,
                    graph_suspension: None,
                };
                let _ = result
                    .reply
                    .ensure_render_frame(&turn_id, &format!("rf-{}", result.steps.len()));
                drop(store);
                drop(runtime);
                return Ok(Json(result));
            }
        }

        // Phase 4.5: deterministic cutover — conductor.root is authority for pure greets/acks.
        if !legacy_spine && aelio_kernel::should_cutover_deterministic(&utterance) {
            let cut = aelio_kernel::run_greeting_cutover(&utterance).map_err(|e| {
                ApiError(AelioError::new(
                    ReasonCode::Internal,
                    format!("greeting cutover failed: {}", e.detail),
                ))
            })?;
            // Durable event admission (dedupe by turn_id as source message).
            {
                let event = aelio_kernel::event_from_user_utterance(
                    &tenant_id,
                    &user_id,
                    &channel,
                    &utterance,
                    &turn_id,
                    "v1.turns.cutover",
                    Some(&turn_id),
                );
                let mut store = os_store.blocking_lock();
                let _ = aelio_kernel::admit_event(store.as_mut(), event, false);
            }
            let mut reply = aelio_agent::abilities::express::Utterance::plain(
                cut.reply_text,
                aelio_agent::abilities::express::ExpressVia::Template,
            );
            reply.template_id = Some("conductor.root.quick_reply".into());
            let mut result = aelio_agent::blocks::turn::TurnResult {
                reply,
                llm_calls: 0,
                tier: None,
                depth: aelio_agent::Depth::Shallow,
                steps: vec![
                    mode_step.clone(),
                    aelio_agent::blocks::turn::TurnTraceStep {
                        name: "Conductor.Cutover".into(),
                        detail: format!(
                            "route={} authority=conductor.root bag_hash={}",
                            cut.route, cut.bag_hash
                        ),
                    },
                    aelio_agent::blocks::turn::TurnTraceStep {
                        name: "Conductor.Select".into(),
                        detail: format!("harness={} — cutover", cut.route),
                    },
                    aelio_agent::blocks::turn::TurnTraceStep {
                        name: "Harness.Load".into(),
                        detail: format!("id=conductor.root route={}", cut.route),
                    },
                    aelio_agent::blocks::turn::TurnTraceStep {
                        name: "Conductor.Shadow".into(),
                        detail: "agent=quick_reply os=quick_reply agree=true comparable=true propose=false cutover=true"
                            .to_string(),
                    },
                ],
                opened_loop: false,
                new_state: None,
                active_flow: None,
                situation_hash: None,
                proposal_id: None,
                suspended: false,
                graph_suspension: None,
            };
            let _ = result
                .reply
                .ensure_render_frame(&turn_id, &format!("rf-{}", result.steps.len()));
            // Do not run the agent spine for cutover turns.
            drop(runtime);
            return Ok(Json(result));
        }

        // Phase 5: installed tool/memory harness cutover (avoids cold ProposePath when present).
        if let Some(intent) =
            (!legacy_spine).then(|| aelio_kernel::resolve_tool_harness_intent(&utterance)).flatten()
        {
            let mut store = os_store.blocking_lock();
            match aelio_kernel::try_run_tool_intent(store.as_mut(), &tenant_id, &intent) {
                Ok(Some((bag, hash, harness_id))) => {
                    // After OTP send, open a multi-turn wait for the code.
                    if harness_id == "tool.send_otp" {
                        if let aelio_kernel::ToolHarnessIntent::SendOtp { phone } = &intent {
                            let _ = aelio_kernel::begin_otp_session_after_send(
                                store.as_mut(),
                                &tenant_id,
                                &user_id,
                                phone,
                            );
                        }
                    }
                    let event = aelio_kernel::event_from_user_utterance(
                        &tenant_id,
                        &user_id,
                        &channel,
                        &utterance,
                        &turn_id,
                        "v1.turns.tool_harness",
                        Some(&turn_id),
                    );
                    let _ = aelio_kernel::admit_event(store.as_mut(), event, false);
                    let mut result = tool_harness_turn_result(&turn_id, harness_id, bag, &hash);
                    result.steps.insert(0, mode_step.clone());
                    if harness_id == "tool.send_otp" {
                        result.reply.text = format!(
                            "{} Enter the 6-digit code (demo: 123456).",
                            result.reply.text.trim_end_matches('.')
                        );
                        result.opened_loop = true;
                        result.suspended = true;
                        result.steps.push(aelio_agent::blocks::turn::TurnTraceStep {
                            name: "Harness.Park".into(),
                            detail: "otp_session waiting for code".into(),
                        });
                    }
                    drop(store);
                    drop(runtime);
                    return Ok(Json(result));
                }
                Ok(None) => {
                    // Not installed — fall through (legacy spine or fail-closed).
                }
                Err(err) => {
                    eprintln!(
                        "aelio-agent-api: tool harness {} failed: {}; falling back",
                        intent.harness_id(),
                        err.detail
                    );
                }
            }
        }

        let legacy = legacy_spine;
        if !legacy {
            // Closed fallback when no harness matched (should be rare with full_reply).
            let mut reply = aelio_agent::abilities::express::Utterance::plain(
                "I could not load a harness for that. Install the vendor library or rephrase.",
                aelio_agent::abilities::express::ExpressVia::Apologize,
            );
            reply.template_id = Some("os.no_harness".into());
            let mut result = aelio_agent::blocks::turn::TurnResult {
                reply,
                llm_calls: 0,
                tier: None,
                depth: aelio_agent::Depth::Shallow,
                steps: vec![
                    mode_step.clone(),
                    aelio_agent::blocks::turn::TurnTraceStep {
                        name: "Conductor.FailClosed".into(),
                        detail: "no installed harness; set AELIO_HARNESS_MODE=legacy to use agent spine"
                            .into(),
                    },
                ],
                opened_loop: false,
                new_state: None,
                active_flow: None,
                situation_hash: None,
                proposal_id: None,
                suspended: false,
                graph_suspension: None,
            };
            let _ = result
                .reply
                .ensure_render_frame(&turn_id, &format!("rf-{}", result.steps.len()));
            drop(runtime);
            return Ok(Json(result));
        }

        runtime
            .run_turn_on_channel(
                DurableTurnRequest {
                    turn_id: request.turn_id.clone(),
                    user_id: request.user_id,
                    utterance: request.utterance,
                },
                &request.channel,
            )
            .map(|mut result| {
                // Phase 4.4: shadow OS Conductor — observation only; agent reply remains authority.
                result.steps.insert(0, mode_step);
                let steps: Vec<(String, String)> = result
                    .steps
                    .iter()
                    .map(|s| (s.name.clone(), s.detail.clone()))
                    .collect();
                let obs = aelio_kernel::observe_shadow(
                    &turn_id,
                    &tenant_id,
                    &user_id,
                    &utterance,
                    &steps,
                    result.suspended,
                );
                {
                    let mut store = os_store.blocking_lock();
                    let _ = aelio_kernel::store_shadow(store.as_mut(), &obs);
                }
                result.steps.push(aelio_agent::blocks::turn::TurnTraceStep {
                    name: "Conductor.Shadow".into(),
                    detail: aelio_kernel::shadow_trace_detail(&obs),
                });
                let _ = result
                    .reply
                    .ensure_render_frame(&turn_id, &format!("rf-{}", result.steps.len()));
                Json(result)
            })
            .map_err(ApiError)
    })
    .await
    .map_err(|error| {
        ApiError(AelioError::new(
            ReasonCode::Internal,
            format!("turn worker failed: {error}"),
        ))
    })?
}

async fn register_catalog(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedTenant>,
    Json(catalog): Json<aelio_agent::tenant::TenantDecl>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if catalog.tenant_id != identity.tenant_id {
        return Err(ApiError(AelioError::new(
            ReasonCode::Denied,
            "catalog tenant does not match authenticated credentials",
        )));
    }
    validate_catalog(&catalog).map_err(ApiError)?;
    let catalog = provision_catalog_artifacts(&state, catalog, &identity.principal)
        .await
        .map_err(ApiError)?;
    let mut runtime = state.runtime.lock().await;
    let tenant_id = catalog.tenant_id.clone();
    let tools = catalog.tools.len();
    let flows = catalog.flows.len();
    runtime.register_catalog(catalog).map_err(ApiError)?;
    let install = catalog_install_status(&runtime, state.artifact_runtime.is_some());
    let sdk_required = state.requires_sdk_bridge && tools > 0;
    let sdk_available = state
        .bridge
        .catalog_status(&tenant_id)
        .is_some_and(|status| status.available);
    let ready = install.executable_pending == 0 && (!sdk_required || sdk_available);
    Ok(Json(serde_json::json!({
        "status": if ready { "active" } else { "installing" },
        "tenant_id": tenant_id,
        "tools": tools,
        "flows": flows,
        "ready": ready,
        "materialization_pending": install.executable_pending,
        "executable_flows": install.executable_flows,
        "semantic_only_flows": install.semantic_only_flows,
    })))
}

/// Admit all catalog tools into the artifact authority before the catalog can become selectable.
/// A partially provisioned set is safe after a crash because proposed/canary artifacts are
/// immutable and rerunning admission is idempotent; the adaptive catalog itself is activated only
/// after every proxy has passed its gate.
async fn provision_catalog_artifacts(
    state: &AppState,
    catalog: TenantDecl,
    deployer: &str,
) -> Result<TenantDecl, AelioError> {
    let Some(runtime) = state.artifact_runtime.clone() else {
        return Ok(catalog);
    };
    let deployer = deployer.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut catalog = catalog;
        for tool in &catalog.tools {
            let effect = tool.effect.ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Validation,
                    format!(
                        "unified catalog tool {} requires an exact pure|read|write|external effect",
                        tool.id
                    ),
                )
            })?;
            let version = tool.version.parse::<u32>().map_err(|_| {
                AelioError::new(
                    ReasonCode::Validation,
                    format!("tool {} version must be a positive integer", tool.id),
                )
            })?;
            if version == 0 || tool.version.starts_with('0') {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!("tool {} version must be canonical and positive", tool.id),
                ));
            }
            runtime
                .install_tool_proxy(
                    aelio_runtime::ToolProxySpec {
                        tenant: catalog.tenant_id.clone(),
                        tool_id: tool.id.clone(),
                        version,
                        effect: match effect {
                            aelio_agent::tenant::ToolEffect::Pure => {
                                aelio_runtime::ArtifactEffect::Pure
                            }
                            aelio_agent::tenant::ToolEffect::Read => {
                                aelio_runtime::ArtifactEffect::Read
                            }
                            aelio_agent::tenant::ToolEffect::Write => {
                                aelio_runtime::ArtifactEffect::Write
                            }
                            aelio_agent::tenant::ToolEffect::External => {
                                aelio_runtime::ArtifactEffect::External
                            }
                        },
                        policy_tags: tool.capability_tags.clone(),
                    },
                    Some(deployer.clone()),
                )
                .map_err(|error| AelioError::new(map_artifact_error(&error), error.to_string()))?;
        }
        flow_lowering::materialize_declared_flows(&runtime, &mut catalog, &deployer)?;
        let consumer = adaptive_bridge::AdaptiveDecisionConsumer::new(runtime);
        for artifact in catalog.flow_artifacts.values() {
            consumer.verify_executable_pin(&catalog.tenant_id, artifact)?;
        }
        for flow in &catalog.flows {
            if catalog.flow_artifacts.contains_key(&flow.id) {
                continue;
            }
            consumer
                .runtime()
                .record_capability_request(
                    &catalog.tenant_id,
                    aelio_runtime::CapabilityRequestDraft {
                        normalized_need: format!(
                            "materialize authored flow {}@{} into one admitted runtime artifact",
                            flow.id, flow.version
                        ),
                        inputs: vec![aelio_runtime::ArtifactInput {
                            name: "turn".into(),
                            imprint: "aelio.turn.input@1".into(),
                            required: true,
                            sensitivity: "internal".into(),
                        }],
                        output: "aelio.turn.output@1".into(),
                        allowed_effects: vec![],
                        requester: "catalog.flow".into(),
                        reason: aelio_runtime::CapabilityReason::InterfaceNotClosed,
                        evidence_refs: vec![],
                    },
                )
                .map_err(|error| AelioError::new(map_artifact_error(&error), error.to_string()))?;
        }
        Ok(catalog)
    })
    .await
    .map_err(|error| {
        AelioError::new(
            ReasonCode::Internal,
            format!("catalog artifact admission worker failed: {error}"),
        )
    })?
}

fn map_artifact_error(error: &aelio_runtime::RuntimeError) -> ReasonCode {
    match error {
        aelio_runtime::RuntimeError::Invalid(_) => ReasonCode::Validation,
        aelio_runtime::RuntimeError::Conflict(_) => ReasonCode::Conflict,
        aelio_runtime::RuntimeError::NotFound(_) => ReasonCode::NotFound,
        aelio_runtime::RuntimeError::Overloaded => ReasonCode::RateLimited,
        aelio_runtime::RuntimeError::Kernel { code, .. } if code.contains("denied") => {
            ReasonCode::Denied
        }
        aelio_runtime::RuntimeError::Kernel { .. } => ReasonCode::ToolError,
        aelio_runtime::RuntimeError::Store(_) | aelio_runtime::RuntimeError::Host(_) => {
            ReasonCode::Unavailable
        }
        aelio_runtime::RuntimeError::Internal(_) => ReasonCode::Internal,
    }
}

async fn active_catalog(State(state): State<AppState>) -> Json<aelio_agent::tenant::TenantDecl> {
    let runtime = state.runtime.lock().await;
    Json(runtime.world.tenant.clone())
}

async fn bind_procedure_artifact(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedTenant>,
    Path(procedure_id): Path<String>,
    Json(artifact): Json<aelio_agent::adaptive::ArtifactPinV1>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let artifact_runtime = state.artifact_runtime.clone().ok_or_else(|| {
        ApiError(AelioError::new(
            ReasonCode::Unavailable,
            "the unified artifact runtime is not configured",
        ))
    })?;
    let tenant_id = identity.tenant_id.clone();
    let verified_artifact = artifact.clone();
    tokio::task::spawn_blocking(move || {
        adaptive_bridge::AdaptiveDecisionConsumer::new(artifact_runtime)
            .verify_atomic_pin(&tenant_id, &verified_artifact)
    })
    .await
    .map_err(|error| {
        ApiError(AelioError::new(
            ReasonCode::Internal,
            format!("artifact verification worker failed: {error}"),
        ))
    })?
    .map_err(ApiError)?;

    let actor = identity.principal;
    let mut runtime = state.runtime.lock().await;
    runtime
        .bind_procedure_artifact(&procedure_id, artifact.clone(), &actor)
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "status":"active",
        "procedure_id":procedure_id,
        "artifact":artifact,
        "binding":"immutable"
    })))
}

async fn bind_flow_artifact(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedTenant>,
    Path(flow_id): Path<String>,
    Json(artifact): Json<aelio_agent::adaptive::ArtifactPinV1>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let artifact_runtime = state.artifact_runtime.clone().ok_or_else(|| {
        ApiError(AelioError::new(
            ReasonCode::Unavailable,
            "the unified artifact runtime is not configured",
        ))
    })?;
    let tenant_id = identity.tenant_id.clone();
    let verified_artifact = artifact.clone();
    tokio::task::spawn_blocking(move || {
        adaptive_bridge::AdaptiveDecisionConsumer::new(artifact_runtime)
            .verify_executable_pin(&tenant_id, &verified_artifact)
    })
    .await
    .map_err(|error| {
        ApiError(AelioError::new(
            ReasonCode::Internal,
            format!("flow artifact verification worker failed: {error}"),
        ))
    })?
    .map_err(ApiError)?;

    let actor = identity.principal;
    let mut runtime = state.runtime.lock().await;
    runtime
        .bind_flow_artifact(&flow_id, artifact.clone(), &actor)
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "status":"active",
        "flow_id":flow_id,
        "artifact":artifact,
        "binding":"immutable"
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetUserStateRequest {
    command_id: String,
    user_id: String,
    state_id: String,
    reason: Option<String>,
}

async fn set_user_state(
    State(state): State<AppState>,
    Json(request): Json<SetUserStateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut runtime = state.runtime.lock().await;
    let replayed = runtime
        .set_user_state(
            &request.command_id,
            &request.user_id,
            &request.state_id,
            request.reason.as_deref(),
        )
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "applied": true,
        "replayed": replayed,
        "user_id": request.user_id,
        "state_id": request.state_id,
    })))
}

async fn configure_exploration(
    State(state): State<AppState>,
    Json(policy): Json<ExplorationPolicy>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut runtime = state.runtime.lock().await;
    runtime
        .configure_exploration(policy.clone())
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "status": "active",
        "enabled": policy.enabled,
        "sample_rate_bps": policy.sample_rate_bps,
        "max_trials_per_window": policy.max_trials_per_window,
    })))
}

#[derive(Debug, Deserialize)]
struct LearningListQuery {
    status: Option<String>,
    limit: Option<usize>,
}

async fn list_learning_proposals(
    State(state): State<AppState>,
    Query(query): Query<LearningListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let rows = runtime
        .list_learning_proposals(query.status.as_deref(), query.limit.unwrap_or(100))
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "items": rows.into_iter().map(|row| serde_json::json!({
            "key": row.envelope.key,
            "status": row.envelope.status,
            "version": row.version,
            "proposal": row.envelope.value,
        })).collect::<Vec<_>>()
    })))
}

#[derive(Debug, Deserialize)]
struct ApprovalRequest {
    approved_by: String,
}

async fn approve_learning_proposal(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedTenant>,
    Path(id): Path<String>,
    Json(request): Json<ApprovalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = format!("{}:{}", identity.principal, request.approved_by);
    let mut runtime = state.runtime.lock().await;
    let result = runtime
        .approve_learning_proposal(&id, &actor)
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "status": "approved",
        "disposition": if matches!(result, PutIfAbsent::Inserted { .. }) {
            "inserted"
        } else {
            "existing"
        }
    })))
}

#[derive(Debug, Deserialize)]
struct PromoteProposalRequest {
    gate: Option<aelio_agent::runtime::PromotionGate>,
}

async fn promote_learning_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PromoteProposalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut runtime = state.runtime.lock().await;
    let result = runtime
        .promote_learning_proposal(&id, &request.gate.unwrap_or_default())
        .map_err(ApiError)?;
    Ok(Json(
        serde_json::json!({"status": "promoted", "result": result}),
    ))
}

async fn list_learning_procedures(
    State(state): State<AppState>,
    Query(query): Query<LearningListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let rows = runtime
        .list_promoted_procedures(query.limit.unwrap_or(100))
        .map_err(ApiError)?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let valid = runtime
            .is_procedure_version_valid(&row.envelope.key)
            .map_err(ApiError)?;
        items.push(serde_json::json!({
            "key": row.envelope.key,
            "status": if valid { "active" } else { "suspended" },
            "version": row.version,
            "procedure": row.envelope.value,
        }));
    }
    Ok(Json(serde_json::json!({
        "items": items
    })))
}

#[derive(Debug, Deserialize)]
struct SuspendProcedureRequest {
    reason: String,
}

async fn suspend_learning_procedure(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SuspendProcedureRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut runtime = state.runtime.lock().await;
    let suspended = runtime
        .suspend_learning_procedure(&id, &request.reason)
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "status": "suspended",
        "versions": suspended,
    })))
}

#[derive(Debug, Deserialize)]
struct FlowCandidateProposalRequest {
    flow: aelio_agent::tenant::FlowSpec,
    source_procedure_versions: Vec<String>,
}

async fn propose_flow_candidate(
    State(state): State<AppState>,
    Json(request): Json<FlowCandidateProposalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut runtime = state.runtime.lock().await;
    let result = runtime
        .propose_candidate_flow(request.flow, request.source_procedure_versions)
        .map_err(ApiError)?;
    let (disposition, row_id, version) = match result {
        PutIfAbsent::Inserted { row_id, version } => ("inserted", row_id, version),
        PutIfAbsent::Existing { row_id, version } => ("existing", row_id, version),
    };
    Ok(Json(serde_json::json!({
        "status": "pending_review",
        "disposition": disposition,
        "row_id": row_id,
        "version": version,
    })))
}

#[derive(Debug, Deserialize)]
struct FlowCandidateListQuery {
    status: Option<String>,
    limit: Option<usize>,
}

async fn list_flow_candidates(
    State(state): State<AppState>,
    Query(query): Query<FlowCandidateListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let rows = runtime
        .list_candidate_flows(query.status.as_deref(), query.limit.unwrap_or(100))
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "items": rows.into_iter().map(|row| serde_json::json!({
            "key": row.envelope.key,
            "status": row.envelope.status,
            "version": row.version,
            "candidate": row.envelope.value,
        })).collect::<Vec<_>>()
    })))
}

#[derive(Debug, Deserialize)]
struct FlowCandidateReviewRequest {
    approved: bool,
    reviewed_by: String,
    reason: Option<String>,
}

async fn review_flow_candidate(
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedTenant>,
    Path(key): Path<String>,
    Json(request): Json<FlowCandidateReviewRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut runtime = state.runtime.lock().await;
    let reviewer = format!("{}:{}", identity.principal, request.reviewed_by);
    runtime
        .review_candidate_flow(&key, request.approved, &reviewer, request.reason)
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "status": if request.approved { "approved" } else { "rejected" }
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlowCandidateActivateRequest {
    #[serde(default)]
    allow_replace: bool,
    /// Required by the unified runtime. Local/parity worlds may omit it while the legacy flow
    /// interpreter remains explicitly enabled.
    artifact: Option<aelio_agent::adaptive::ArtifactPinV1>,
}

async fn activate_flow_candidate(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(request): Json<FlowCandidateActivateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut runtime = state.runtime.lock().await;
    runtime
        .activate_candidate_flow_with_artifact(&key, request.allow_replace, request.artifact)
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({"status": "active"})))
}

async fn sdk_socket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(identity): Extension<AuthenticatedTenant>,
) -> impl IntoResponse {
    ws.max_message_size(1024 * 1024)
        .max_frame_size(1024 * 1024)
        .on_upgrade(move |socket| handle_sdk_socket(socket, state, identity.tenant_id))
}

async fn handle_sdk_socket(socket: WebSocket, state: AppState, tenant_id: String) {
    let connection_id = Uuid::new_v4().to_string();
    let mut outbound = state.bridge.open_connection(connection_id.clone());
    let (mut sender, mut receiver) = socket.split();

    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let Some(Ok(message)) = incoming else {
                    break;
                };
                match message {
                    Message::Text(text) => {
                        let response = handle_sdk_message(
                            &state,
                            &connection_id,
                            &tenant_id,
                            text.as_str(),
                        ).await;
                        if let Some(response) = response {
                            if sender.send(Message::Text(response.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        if sender.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Message::Binary(_) | Message::Pong(_) => {}
                }
            }
            invocation = outbound.recv() => {
                let Some(invocation) = invocation else {
                    break;
                };
                let invocation_id = invocation.invocation_id.clone();
                let Ok(encoded) = serde_json::to_string(&invocation) else {
                    break;
                };
                if sender.send(Message::Text(encoded.into())).await.is_err() {
                    break;
                }
                if state
                    .bridge
                    .mark_dispatched(&connection_id, &invocation_id)
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    state.bridge.disconnect(&connection_id);
}

async fn handle_sdk_message(
    state: &AppState,
    connection_id: &str,
    tenant_id: &str,
    text: &str,
) -> Option<String> {
    let parsed: serde_json::Value = match serde_json::from_str(text) {
        Ok(parsed) => parsed,
        Err(_) => return Some(wire_error("invalid_json", "message was not valid JSON")),
    };
    match parsed.get("type").and_then(serde_json::Value::as_str) {
        Some("register") => {
            let mut registration: RegisterMessage = match serde_json::from_value(parsed) {
                Ok(registration) => registration,
                Err(error) => {
                    return Some(wire_error(
                        "invalid_registration",
                        &format!("registration failed schema validation: {error}"),
                    ));
                }
            };
            if registration.message_type != "register" {
                return Some(wire_error("invalid_registration", "invalid message type"));
            }
            if registration.catalog.tenant_id != tenant_id {
                return Some(wire_error(
                    "denied",
                    "catalog tenant does not match authenticated credentials",
                ));
            }
            if let Err(error) = validate_catalog(&registration.catalog) {
                return Some(wire_aelio_error(&error));
            }
            registration.catalog = match provision_catalog_artifacts(
                state,
                registration.catalog,
                &format!("sdk-connection:{connection_id}"),
            )
            .await
            {
                Ok(catalog) => catalog,
                Err(error) => return Some(wire_aelio_error(&error)),
            };
            {
                let mut runtime = state.runtime.lock().await;
                if let Err(error) = runtime.register_catalog(registration.catalog.clone()) {
                    return Some(wire_aelio_error(&error));
                }
            }
            match state
                .bridge
                .register(connection_id, tenant_id, registration.catalog)
            {
                Ok(ack) => serde_json::to_string(&ack).ok(),
                Err(error) => Some(wire_aelio_error(&error)),
            }
        }
        Some("result") => {
            match serde_json::from_value::<ResultMessage>(parsed) {
                Ok(result) => {
                    let invocation_id = result.invocation_id.clone();
                    match state.bridge.complete(connection_id, result) {
                        Ok(disposition) => {
                            let disposition = match disposition {
                                aelio_wire::ResultDisposition::Accepted => "accepted",
                                aelio_wire::ResultDisposition::Duplicate => "duplicate",
                                aelio_wire::ResultDisposition::Late => "late",
                                aelio_wire::ResultDisposition::UnknownCorrelation => "unknown",
                            };
                            if disposition != "unknown" {
                                let mut runtime = state.runtime.lock().await;
                                if let Err(error) = runtime
                                    .record_sdk_delivery_disposition(&invocation_id, disposition)
                                {
                                    return Some(wire_aelio_error(&error));
                                }
                            }
                            let delivery_ledger =
                                state.bridge.delivery_trace(&invocation_id).ok().and_then(
                                    |trace| serde_json::from_str::<serde_json::Value>(&trace).ok(),
                                );
                            Some(
                                serde_json::json!({
                                    "type":"result_ack",
                                    "invocation_id":invocation_id,
                                    "disposition":disposition,
                                    "delivery_ledger":delivery_ledger,
                                })
                                .to_string(),
                            )
                        }
                        Err(error) => Some(wire_aelio_error(&error)),
                    }
                }
                Err(error) => Some(wire_error(
                    "invalid_result",
                    &format!("result failed schema validation: {error}"),
                )),
            }
        }
        Some("pong") => None,
        Some(_) => Some(wire_error(
            "unsupported_message",
            "unsupported SDK message type",
        )),
        None => Some(wire_error("invalid_message", "message type is required")),
    }
}

async fn list_sdk_delivery_audit(
    State(state): State<AppState>,
    Query(query): Query<LearningListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let rows = runtime
        .list_sdk_delivery_dispositions(query.limit.unwrap_or(100))
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "items": rows.into_iter().map(|row| serde_json::json!({
            "key":row.envelope.key,
            "status":row.envelope.status,
            "version":row.version,
            "event":row.envelope.value,
        })).collect::<Vec<_>>()
    })))
}

async fn list_tool_outcomes(
    State(state): State<AppState>,
    Query(query): Query<LearningListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let rows = runtime
        .list_tool_outcomes(query.status.as_deref(), query.limit.unwrap_or(100))
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "items":rows.into_iter().map(|row| {
            let idempotency_hash = hex::encode(Sha256::digest(row.envelope.value.key.as_bytes()));
            let subject_hash = hex::encode(Sha256::digest(row.envelope.value.user_id.as_bytes()));
            serde_json::json!({
                "idempotency_hash":idempotency_hash,
                "subject_hash":subject_hash,
                "tool_id":row.envelope.value.tool_id,
                "tool_version":row.envelope.value.tool_version,
                "state":row.envelope.value.state,
                "attempts":row.envelope.value.attempts,
                "reason_code":row.envelope.value.reason_code,
                "version":row.version,
            })
        }).collect::<Vec<_>>()
    })))
}

async fn list_open_loops(
    State(state): State<AppState>,
    Query(query): Query<LearningListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let rows = runtime
        .list_open_loops(query.limit.unwrap_or(100))
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "items":rows.into_iter().map(|row| {
            let subject_hash = hex::encode(Sha256::digest(row.envelope.owner.as_bytes()));
            let intent_hash = hex::encode(Sha256::digest(row.envelope.value.text.as_bytes()));
            serde_json::json!({
                "subject_hash":subject_hash,
                "intent_hash":intent_hash,
                "state":row.envelope.value.open_loop_state,
                "created_at_ms":row.envelope.created_at_ms,
                "version":row.version,
            })
        }).collect::<Vec<_>>()
    })))
}

fn wire_aelio_error(error: &AelioError) -> String {
    wire_error(&format!("{:?}", error.code).to_lowercase(), &error.message)
}

fn wire_error(code: &str, message: &str) -> String {
    serde_json::json!({
        "type": "error",
        "code": code,
        "message": message,
    })
    .to_string()
}

async fn flush(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let mut runtime = state.runtime.lock().await;
    runtime.store.flush().map_err(ApiError)?;
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn schedule_job(
    State(state): State<AppState>,
    Json(job): Json<ScheduledJob>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let tenant_id = runtime.world.tenant.tenant_id.clone();
    let mut scheduler = DurableScheduler::new(tenant_id, runtime.store.clone());
    let now_ms = chrono::Utc::now().timestamp_millis();
    let result = scheduler.schedule(job, now_ms).map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "scheduled": matches!(result, aelio_agent::storage::PutIfAbsent::Inserted { .. })
    })))
}

#[derive(Debug, Deserialize)]
struct ProactiveRequest {
    candidate: ProactiveCandidate,
    policy: ProactivePolicy,
    now_ms: Option<i64>,
}

async fn evaluate_proactive(
    State(state): State<AppState>,
    Json(request): Json<ProactiveRequest>,
) -> Result<Json<ProactiveDecision>, ApiError> {
    let runtime = state.runtime.lock().await;
    let tenant_id = runtime.world.tenant.tenant_id.clone();
    let store = runtime.store.clone();
    drop(runtime);
    let now_ms = request
        .now_ms
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let mut proactive = ProactiveLoop::new(tenant_id, store);
    proactive
        .evaluate_and_enqueue(request.candidate, &request.policy, now_ms)
        .map(Json)
        .map_err(ApiError)
}

#[derive(Debug, Deserialize)]
struct WorkerTickRequest {
    owner: String,
    kind: Option<String>,
    #[serde(default = "default_lease_ms")]
    lease_ms: i64,
    #[serde(default = "default_worker_limit")]
    limit: usize,
    now_ms: Option<i64>,
}

fn default_lease_ms() -> i64 {
    30_000
}

fn default_worker_limit() -> usize {
    32
}

async fn worker_tick(
    State(state): State<AppState>,
    Json(request): Json<WorkerTickRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let tenant_id = runtime.world.tenant.tenant_id.clone();
    let store = runtime.store.clone();
    drop(runtime);
    let now_ms = request
        .now_ms
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let mut scheduler = DurableScheduler::new(tenant_id.clone(), store.clone());
    let leased = scheduler
        .lease_due_kind(
            &request.owner,
            now_ms,
            request.lease_ms,
            request.limit.min(256),
            request.kind.as_deref(),
        )
        .map_err(ApiError)?;
    let mut reconciler = SagaReconciler::new(tenant_id, store);
    let reconciled = reconciler
        .reconcile(now_ms, request.limit.min(256))
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "leased_jobs": leased.into_iter().map(|lease| lease.job).collect::<Vec<_>>(),
        "reconciled": reconciled,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerCompleteRequest {
    owner: String,
    job_id: String,
    now_ms: Option<i64>,
}

async fn worker_complete(
    State(state): State<AppState>,
    Json(request): Json<WorkerCompleteRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state.runtime.lock().await;
    let tenant_id = runtime.world.tenant.tenant_id.clone();
    let store = runtime.store.clone();
    drop(runtime);
    let now_ms = request
        .now_ms
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    DurableScheduler::new(tenant_id, store)
        .complete_owned(&request.job_id, &request.owner, now_ms)
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({"completed": true})))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerFailRequest {
    owner: String,
    job_id: String,
    reason: String,
    retry_in_ms: i64,
    now_ms: Option<i64>,
}

async fn worker_fail(
    State(state): State<AppState>,
    Json(request): Json<WorkerFailRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.reason.trim().is_empty()
        || request.reason.len() > 2_048
        || !(0..=86_400_000).contains(&request.retry_in_ms)
    {
        return Err(ApiError(AelioError::new(
            ReasonCode::Validation,
            "bounded failure reason and retry interval are required",
        )));
    }
    let runtime = state.runtime.lock().await;
    let tenant_id = runtime.world.tenant.tenant_id.clone();
    let store = runtime.store.clone();
    drop(runtime);
    let now_ms = request
        .now_ms
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let state = DurableScheduler::new(tenant_id, store)
        .fail_owned(
            &request.job_id,
            &request.owner,
            now_ms,
            now_ms.saturating_add(request.retry_in_ms),
            request.reason,
        )
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({"failed": true, "state": state})))
}

async fn authenticate(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    if state.is_open() {
        if request.uri().path().starts_with("/v1/admin/") {
            return Err(ApiError(AelioError::new(
                ReasonCode::Denied,
                "admin routes require AELIO_API_KEYS even in open development mode",
            )));
        }
        let tenant_id = state.runtime.lock().await.world.tenant.tenant_id.clone();
        let mut request = request;
        request.extensions_mut().insert(AuthenticatedTenant {
            tenant_id,
            principal: "open-development-mode".into(),
        });
        return Ok(next.run(request).await);
    }
    let token = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "));
    if let Some(credential) = token
        .and_then(|token| state.credentials.get(token))
        .cloned()
    {
        if request.uri().path().starts_with("/v1/admin/") && !credential.admin {
            return Err(ApiError(AelioError::new(
                ReasonCode::Denied,
                "administrative credential required",
            )));
        }
        let mut request = request;
        request.extensions_mut().insert(AuthenticatedTenant {
            tenant_id: credential.tenant_id,
            principal: credential.principal,
        });
        Ok(next.run(request).await)
    } else {
        Err(ApiError(AelioError::new(
            ReasonCode::Denied,
            "invalid or missing bearer token",
        )))
    }
}

struct ApiError(AelioError);

#[derive(Serialize)]
struct ErrorBody {
    code: ReasonCode,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.code {
            ReasonCode::Validation | ReasonCode::ParseError => StatusCode::BAD_REQUEST,
            ReasonCode::Denied | ReasonCode::PolicyDenied => StatusCode::UNAUTHORIZED,
            ReasonCode::NotFound => StatusCode::NOT_FOUND,
            ReasonCode::Conflict => StatusCode::CONFLICT,
            ReasonCode::RateLimited | ReasonCode::BudgetExceeded => StatusCode::TOO_MANY_REQUESTS,
            ReasonCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ReasonCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorBody {
                code: self.0.code,
                message: self.0.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod constructor_authority_tests {
    use super::*;
    use aelio_agent::runtime::World;
    use aelio_agent::storage::AelioStore;
    use aelio_db_query::Database;

    fn demo_runtime(tag: &str) -> DurableRuntime {
        let path =
            std::env::temp_dir().join(format!("aelio_appstate_ctor_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let store = AelioStore::new(Database::create(path).unwrap(), 3).unwrap();
        let world = World::demo_tenant("tenant-1");
        assert!(
            world.legacy_flow_execution_enabled,
            "demo worlds start with legacy enabled so the constructor must clear it"
        );
        DurableRuntime::new(world, store).unwrap()
    }

    #[test]
    fn every_appstate_constructor_disables_legacy_flow_execution() {
        let sdk = AppState::new_with_sdk_bridge(
            demo_runtime("sdk"),
            vec!["k".into()],
            BridgeConfig::default(),
        );
        assert!(
            !sdk.runtime
                .blocking_lock()
                .world
                .legacy_flow_execution_enabled
        );

        let scoped = AppState::new_with_scoped_sdk_bridge(
            demo_runtime("scoped"),
            vec!["k".into()],
            vec!["admin".into()],
            BridgeConfig::default(),
        );
        assert!(
            !scoped
                .runtime
                .blocking_lock()
                .world
                .legacy_flow_execution_enabled
        );

        let scoped_parity = AppState::new_with_scoped_sdk_bridge_for_legacy_parity(
            demo_runtime("scoped-parity"),
            vec!["k".into()],
            vec!["admin".into()],
            BridgeConfig::default(),
        );
        assert!(
            scoped_parity
                .runtime
                .blocking_lock()
                .world
                .legacy_flow_execution_enabled
        );

        let plain = AppState::new(demo_runtime("plain"), vec!["k".into()]);
        assert!(
            !plain
                .runtime
                .blocking_lock()
                .world
                .legacy_flow_execution_enabled
        );

        let parity = AppState::new_for_legacy_parity(demo_runtime("parity"), vec!["k".into()]);
        assert!(
            parity
                .runtime
                .blocking_lock()
                .world
                .legacy_flow_execution_enabled
        );
    }
}
