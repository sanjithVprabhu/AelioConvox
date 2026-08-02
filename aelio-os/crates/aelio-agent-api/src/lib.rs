//! Rust HTTP boundary for the durable Aelio runtime.

pub mod adaptive_bridge;
mod flow_lowering;
mod runtime_adaptive_host;
mod runtime_tool_host;
pub mod sdk_bridge;

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
        )
    }

    /// Unified production constructor: the adaptive agent selects capabilities, but every tool
    /// effect executes through a gated `aelio-runtime` proxy artifact.
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
        )
    }

    /// Production constructor: installs the SDK-backed `ToolHost` used by the generic executor.
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
        )
    }

    /// Production constructor with distinct runtime/SDK and administrative credentials.
    pub fn new_with_scoped_sdk_bridge(
        runtime: DurableRuntime,
        api_keys: Vec<String>,
        admin_api_keys: Vec<String>,
        bridge_config: BridgeConfig,
    ) -> Self {
        Self::build(runtime, api_keys, admin_api_keys, bridge_config, true, None)
    }

    fn build(
        mut runtime: DurableRuntime,
        api_keys: Vec<String>,
        admin_api_keys: Vec<String>,
        bridge_config: BridgeConfig,
        install_host: bool,
        artifact_runtime: Option<aelio_runtime::Runtime>,
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
        let bridge = SdkBridge::new(bridge_config);
        if install_host {
            runtime.install_tool_host(Box::new(bridge.tool_host(tenant_id)));
        } else if let Some(artifact_runtime) = &artifact_runtime {
            runtime.world.disable_legacy_flow_execution();
            runtime.install_tool_host(Box::new(runtime_tool_host::RuntimeArtifactToolHost::new(
                artifact_runtime.clone(),
                tenant_id.clone(),
            )));
            runtime.world.set_adaptive_artifact_host(Box::new(
                runtime_adaptive_host::RuntimeAdaptiveArtifactHost::new(artifact_runtime.clone()),
            ));
        }
        Self {
            runtime: Arc::new(Mutex::new(runtime)),
            credentials: Arc::new(credentials),
            bridge,
            requires_sdk_bridge: install_host,
            artifact_runtime,
        }
    }

    pub fn is_open(&self) -> bool {
        self.credentials.is_empty()
    }

    pub fn bridge(&self) -> SdkBridge {
        self.bridge.clone()
    }
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/turns", post(process_turn))
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
    let unmaterialized_flows = if state.artifact_runtime.is_some() {
        runtime
            .world
            .tenant
            .flows
            .iter()
            .filter(|flow| runtime.world.registry.flow_artifact(&flow.id).is_none())
            .count()
    } else {
        0
    };
    let sdk_required = state.requires_sdk_bridge && !runtime.world.tenant.tools.is_empty();
    let sdk = state.bridge.catalog_status(&tenant_id);
    let sdk_registered = sdk.is_some();
    let sdk_available = sdk.is_some_and(|status| status.available);
    let ready = catalog_registered && unmaterialized_flows == 0 && (!sdk_required || sdk_available);
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
        "unmaterialized_flows": unmaterialized_flows,
        "sdk_required": sdk_required,
        "sdk_registered": sdk_registered,
        "sdk_available": sdk_available,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnApiRequest {
    turn_id: String,
    user_id: String,
    utterance: String,
    #[serde(default = "unknown_channel")]
    channel: String,
}

fn unknown_channel() -> String {
    "unknown".into()
}

async fn process_turn(
    State(state): State<AppState>,
    Json(request): Json<TurnApiRequest>,
) -> Result<Json<aelio_agent::blocks::turn::TurnResult>, ApiError> {
    let runtime = Arc::clone(&state.runtime);
    tokio::task::spawn_blocking(move || {
        let mut runtime = runtime.blocking_lock();
        if runtime.world.tenant.states.is_empty() || runtime.world.tenant.personalities.is_empty() {
            return Err(ApiError(AelioError::new(
                ReasonCode::Unavailable,
                "tenant catalog is not registered",
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
                let _ = result
                    .reply
                    .ensure_render_frame(&request.turn_id, &format!("rf-{}", result.steps.len()));
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
    let materialization_pending = runtime
        .world
        .tenant
        .flows
        .iter()
        .filter(|flow| runtime.world.registry.flow_artifact(&flow.id).is_none())
        .count();
    Ok(Json(serde_json::json!({
        "status": "active",
        "tenant_id": tenant_id,
        "tools": tools,
        "flows": flows,
        "materialization_pending": materialization_pending,
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
