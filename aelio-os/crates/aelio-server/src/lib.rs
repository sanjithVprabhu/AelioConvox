//! HTTP boundary for the authoritative Rust runtime. This service owns flow validation,
//! execution, continuations, ledgers, and persistence. The TypeScript process is a host adapter.

pub mod agent_host;

use aelio_prompt::MintRequest;
use aelio_runtime::{
    baseline_conversation_flow, forge_flow, mint_prompt, ArtifactEffect, ArtifactStatus,
    BuildReactionOutput, BuildReactionUsage, BuildSpec, BuildSpecDraft, CanaryObservation,
    CapabilityRequestDraft, FlowPush, ForgeRequest, MockDrafter, MockMintDrafter, PromptGateCase,
    Runtime, RuntimeError, SandboxCase, SandboxFixtureCall, SandboxLimits, TurnSubmit,
    BASELINE_CONVERSATION_FLOW_ID, BASELINE_CONVERSATION_FLOW_REV,
};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct ServerState {
    runtime: Runtime,
    api_tokens: Arc<[String]>,
    allow_insecure: bool,
}

impl ServerState {
    pub fn new(
        runtime: Runtime,
        api_tokens: Vec<String>,
        allow_insecure: bool,
    ) -> Result<Self, String> {
        if api_tokens.is_empty() && !allow_insecure {
            return Err(
                "AELIO_RUNTIME_TOKENS is required; use AELIO_ALLOW_INSECURE_OPEN=1 only for local development"
                    .into(),
            );
        }
        if api_tokens.iter().any(|token| token.len() < 16) {
            return Err("runtime API tokens must contain at least 16 characters".into());
        }
        Ok(Self {
            runtime,
            api_tokens: api_tokens.into(),
            allow_insecure,
        })
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        if self.allow_insecure {
            return true;
        }
        let Some(candidate) = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return false;
        };
        self.api_tokens
            .iter()
            .any(|expected| constant_time_equal(candidate.as_bytes(), expected.as_bytes()))
    }
}

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/flows", post(push_flow))
        .route("/v1/flows/gate", post(gate_flow_http))
        .route("/v1/artifacts/canary-evidence", post(canary_evidence_http))
        .route(
            "/v1/artifacts/promotion-proposals/{id}/apply",
            post(apply_promotion_http),
        )
        .route(
            "/v1/capability-requests",
            get(capability_requests_http).post(record_capability_request_http),
        )
        .route(
            "/v1/capability-requests/{id}/queue",
            post(queue_capability_build_http),
        )
        .route(
            "/v1/capability-requests/{id}",
            get(get_capability_request_http),
        )
        .route("/v1/builds", post(submit_build_http))
        .route("/v1/builds/{id}", get(get_build_http))
        .route("/v1/builds/{id}/advance", post(advance_build_http))
        .route("/v1/builds/{id}/run", post(run_build_http))
        .route("/v1/builds/{id}/reaction", post(build_reaction_http))
        .route("/v1/flows/forge", post(forge_flow_http))
        .route("/v1/mint", post(mint_http))
        .route("/v1/mint/gate", post(mint_gate_http))
        .route("/v1/mint/recall", post(mint_recall_http))
        .route(
            "/v1/system/flows/conversation",
            post(install_baseline_conversation),
        )
        .route("/v1/turns", post(submit_turn))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineFlowRequest {
    tenant: String,
}

async fn install_baseline_conversation(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<BaselineFlowRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let principal = authenticated_principal(&state, &headers)?;
    let flow = baseline_conversation_flow(&request.tenant);
    state
        .runtime
        .install_baseline_conversation(&request.tenant)
        .map_err(ApiError::from)?;
    let cases: Vec<_> = (0..20)
        .map(|index| SandboxCase {
            input: serde_json::json!({"message":format!("bootstrap validation {index}")}),
            wakes: vec![],
            expect_park: true,
            expected: serde_json::json!({"answer":{"text":"sandbox-ok"}}),
            fixtures: vec![SandboxFixtureCall {
                target: "aelio.model.complete@1".into(),
                expected_args_hash: None,
                output: serde_json::json!({"text":"sandbox-ok"}),
                usage_tokens: 1,
            }],
        })
        .collect();
    let current = state
        .runtime
        .artifact_repository()
        .map_err(ApiError::from)?
        .get(
            &request.tenant,
            BASELINE_CONVERSATION_FLOW_ID,
            BASELINE_CONVERSATION_FLOW_REV
                .parse()
                .expect("constant version"),
        )
        .map_err(|error| ApiError::from(RuntimeError::Internal(error.to_string())))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "baseline artifact disappeared after installation",
            )
        })?;
    let status = if matches!(
        current.status,
        ArtifactStatus::Canary | ArtifactStatus::Promoted
    ) {
        current.status
    } else {
        state
            .runtime
            .gate_flow(&flow, &cases, SandboxLimits::default(), principal)
            .map_err(ApiError::from)?
            .record
            .status
    };
    if !matches!(status, ArtifactStatus::Canary | ArtifactStatus::Promoted) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "approval_required",
            "baseline conversation is reviewed-tier and requires an authenticated deployer",
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "accepted": true,
            "flow_id": BASELINE_CONVERSATION_FLOW_ID,
            "flow_rev": BASELINE_CONVERSATION_FLOW_REV,
            "status": status,
        })),
    ))
}

async fn health() -> Json<StatusBody> {
    Json(StatusBody {
        status: "ok",
        service: "aelio-server",
        authority: "rust",
    })
}

async fn ready(State(state): State<ServerState>) -> Response {
    if state.api_tokens.is_empty() && !state.allow_insecure {
        return ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "not_ready",
            "authentication is not configured",
        )
        .into_response();
    }
    Json(StatusBody {
        status: "ready",
        service: "aelio-server",
        authority: "rust",
    })
    .into_response()
}

async fn push_flow(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<FlowPush>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    state.runtime.push_flow(request).map_err(ApiError::from)?;
    Ok((StatusCode::CREATED, Json(Ack { accepted: true })))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GateFlowRequest {
    flow: FlowPush,
    cases: Vec<SandboxCase>,
    #[serde(default)]
    limits: Option<SandboxLimits>,
}

async fn gate_flow_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<GateFlowRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let principal = authenticated_principal(&state, &headers)?;
    let result = state
        .runtime
        .gate_flow(
            &request.flow,
            &request.cases,
            request.limits.unwrap_or_default(),
            principal,
        )
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CanaryEvidenceRequest {
    tenant: String,
    artifact_id: String,
    artifact_version: u32,
    observation: CanaryObservation,
}

async fn canary_evidence_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<CanaryEvidenceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    let result = state
        .runtime
        .observe_canary(
            &request.tenant,
            &request.artifact_id,
            request.artifact_version,
            request.observation,
        )
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantRequest {
    tenant: String,
}

async fn apply_promotion_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<TenantRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    state
        .runtime
        .apply_promotion_proposal(&request.tenant, &id)
        .map(Json)
        .map_err(ApiError::from)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DemandQuery {
    tenant: String,
    #[serde(default = "default_demand_limit")]
    limit: usize,
}

fn default_demand_limit() -> usize {
    100
}

async fn capability_requests_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<DemandQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    state
        .runtime
        .list_capability_requests(&query.tenant, query.limit)
        .map(Json)
        .map_err(ApiError::from)
}

async fn get_capability_request_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<BuildTenantQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    state
        .runtime
        .get_capability_request(&query.tenant, &id)
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "capability request not found",
            )
        })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordCapabilityRequest {
    tenant: String,
    draft: CapabilityRequestDraft,
}

async fn record_capability_request_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut request): Json<RecordCapabilityRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let principal = authenticated_principal(&state, &headers)?.ok_or_else(|| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "approval_required",
            "authentication required",
        )
    })?;
    request.draft.requester = principal;
    let demand = state
        .runtime
        .record_capability_request(&request.tenant, request.draft)
        .map_err(ApiError::from)?;
    Ok((StatusCode::ACCEPTED, Json(demand)))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueCapabilityBuildRequest {
    tenant: String,
    spec: BuildSpecDraft,
}

async fn queue_capability_build_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut request): Json<QueueCapabilityBuildRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    request.spec.policy.principal_grants = vec![
        ArtifactEffect::Pure,
        ArtifactEffect::Read,
        ArtifactEffect::Write,
        ArtifactEffect::External,
    ];
    let spec = BuildSpec::seal(request.spec)
        .map_err(|error| ApiError::from(RuntimeError::Invalid(error.to_string())))?;
    let queued = state
        .runtime
        .queue_capability_build(&request.tenant, &id, spec)
        .map_err(ApiError::from)?;
    Ok((StatusCode::ACCEPTED, Json(queued)))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitBuildRequest {
    spec: BuildSpecDraft,
}

async fn submit_build_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut request): Json<SubmitBuildRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    // Runtime tokens are deployer credentials in v1. Never trust a caller's self-declared grants;
    // derive the grant ceiling at this authenticated boundary, then seal the canonical spec hash.
    request.spec.policy.principal_grants = vec![
        ArtifactEffect::Pure,
        ArtifactEffect::Read,
        ArtifactEffect::Write,
        ArtifactEffect::External,
    ];
    let spec = BuildSpec::seal(request.spec)
        .map_err(|error| ApiError::from(RuntimeError::Invalid(error.to_string())))?;
    let job = state.runtime.submit_build(spec).map_err(ApiError::from)?;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildTenantQuery {
    tenant: String,
}

async fn get_build_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<BuildTenantQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    let job = state
        .runtime
        .get_build(&query.tenant, &id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_found", "build not found"))?;
    Ok(Json(job))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AdvanceBuildRequest {
    tenant: String,
}

async fn advance_build_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<AdvanceBuildRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let principal = authenticated_principal(&state, &headers)?;
    let action = state
        .runtime
        .advance_build(&request.tenant, &id, principal)
        .map_err(ApiError::from)?;
    Ok(Json(action))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RunBuildRequest {
    tenant: String,
    #[serde(default = "default_build_boundaries")]
    max_boundaries: usize,
}

fn default_build_boundaries() -> usize {
    512
}

async fn run_build_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<RunBuildRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let principal = authenticated_principal(&state, &headers)?;
    if !state.runtime.host_adapter_configured() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "host_unavailable",
            "build model worker requires the configured TypeScript host adapter",
        ));
    }
    if request.max_boundaries == 0 || request.max_boundaries > 4_096 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "max_boundaries must be 1..=4096",
        ));
    }
    let runtime = state.runtime.clone();
    let tenant = request.tenant;
    let build_id = id.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) =
            runtime.run_build_with_host(&tenant, &build_id, principal, request.max_boundaries)
        {
            eprintln!("build worker {build_id} stopped: {error}");
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"accepted":true,"build_id":id})),
    ))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildReactionRequest {
    tenant: String,
    reaction_id: String,
    output: BuildReactionOutput,
    #[serde(default)]
    usage: BuildReactionUsage,
}

async fn build_reaction_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<BuildReactionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    let job = state
        .runtime
        .submit_build_reaction(
            &request.tenant,
            &id,
            &request.reaction_id,
            request.output,
            request.usage,
        )
        .map_err(ApiError::from)?;
    Ok(Json(job))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgeHttpRequest {
    #[serde(flatten)]
    forge: ForgeRequest,
    /// `mock` (default) or `host`. `openai` remains a compatibility alias for `host`.
    #[serde(default = "default_drafter")]
    drafter: String,
}

fn default_drafter() -> String {
    "mock".into()
}

async fn forge_flow_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<ForgeHttpRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    let result = match request.drafter.as_str() {
        "mock" => forge_flow(Some(&state.runtime), &request.forge, &MockDrafter)
            .map_err(ApiError::from)?,
        "host" | "openai" => state
            .runtime
            .forge_flow_with_host(&request.forge)
            .map_err(ApiError::from)?,
        other => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!("unknown drafter `{other}`; use mock|host"),
            ));
        }
    };
    let status = if result.stored {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(result)))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MintHttpRequest {
    mint: MintRequest,
    #[serde(default = "default_drafter")]
    drafter: String,
    #[serde(default = "default_true")]
    store: bool,
}

fn default_true() -> bool {
    true
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MintRecallRequest {
    tenant: String,
    need: String,
    #[serde(default = "default_recall_limit")]
    limit: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MintGateRequest {
    tenant: String,
    id: String,
    version: u32,
    cases: Vec<PromptGateCase>,
}

fn default_recall_limit() -> u64 {
    10
}

async fn mint_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<MintHttpRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    let result = match request.drafter.as_str() {
        "mock" => {
            let shelf = request
                .store
                .then(|| state.runtime.mint_shelf())
                .transpose()
                .map_err(ApiError::from)?;
            mint_prompt(shelf.as_ref(), &request.mint, &MockMintDrafter).map_err(ApiError::from)?
        }
        "host" | "openai" => state
            .runtime
            .mint_prompt_with_host(&request.mint, request.store)
            .map_err(ApiError::from)?,
        other => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid",
                format!("unknown drafter `{other}`; use mock|host"),
            ));
        }
    };
    let status = if result.stored {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(result)))
}

async fn mint_gate_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<MintGateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let principal = authenticated_principal(&state, &headers)?.ok_or_else(|| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "approval_required",
            "prompt admission requires an authenticated deployer; insecure mode cannot approve",
        )
    })?;
    let shelf = state.runtime.mint_shelf().map_err(ApiError::from)?;
    let prompt = shelf
        .get(
            &request.tenant,
            &format!("{}@{}", request.id, request.version),
        )
        .map_err(ApiError::from)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "minted prompt artifact not found in tenant namespace",
            )
        })?;
    let result = state
        .runtime
        .gate_prompt_with_host(&request.tenant, &prompt, &request.cases, Some(principal))
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn mint_recall_http(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<MintRecallRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    if request.need.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid",
            "need must not be empty",
        ));
    }
    let shelf = state.runtime.mint_shelf().map_err(ApiError::from)?;
    let hits = shelf
        .recall(
            &request.tenant,
            request.need.trim(),
            aelio_query::RecallModality::Hybrid,
            request.limit.clamp(1, 100),
        )
        .map_err(ApiError::from)?;
    let rows: Vec<_> = hits
        .into_iter()
        .map(|h| {
            serde_json::json!({
                "row_id": h.row_id,
                "score": h.score,
                "fields": h.fields.iter().map(|(k,v)| (k, format!("{v:?}"))).collect::<BTreeMap<_,_>>(),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "hits": rows })))
}

async fn submit_turn(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<TurnSubmit>,
) -> Result<impl IntoResponse, ApiError> {
    require_auth(&state, &headers)?;
    let response = state
        .runtime
        .submit(request)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(response))
}

fn require_auth(state: &ServerState, headers: &HeaderMap) -> Result<(), ApiError> {
    if state.authorized(headers) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer token is required",
        ))
    }
}

/// Return a non-secret stable actor id for reviewed-tier approval. In insecure development mode
/// the request may proceed, but it can never count as deployer approval.
fn authenticated_principal(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<String>, ApiError> {
    let candidate = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if let Some(candidate) = candidate {
        if state
            .api_tokens
            .iter()
            .any(|expected| constant_time_equal(candidate.as_bytes(), expected.as_bytes()))
        {
            let digest = blake3::hash(candidate.as_bytes()).to_hex();
            return Ok(Some(format!("api-key-{}", &digest[..16])));
        }
    }
    if state.allow_insecure {
        Ok(None)
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer token is required",
        ))
    }
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[derive(Serialize)]
struct StatusBody {
    status: &'static str,
    service: &'static str,
    authority: &'static str,
}

#[derive(Serialize)]
struct Ack {
    accepted: bool,
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
    detail: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status,
            code,
            detail: detail.into(),
        }
    }
}

impl From<RuntimeError> for ApiError {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::Invalid(detail) => Self::new(StatusCode::BAD_REQUEST, "invalid", detail),
            RuntimeError::Conflict(detail) => Self::new(StatusCode::CONFLICT, "conflict", detail),
            RuntimeError::NotFound(detail) => Self::new(StatusCode::NOT_FOUND, "not_found", detail),
            RuntimeError::Overloaded => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "overloaded",
                "instance queue is full",
            ),
            RuntimeError::Kernel { code, detail } => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "kernel",
                format!("{code}: {detail}"),
            ),
            RuntimeError::Store(detail) => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "store", detail)
            }
            RuntimeError::Host(detail) => {
                Self::new(StatusCode::BAD_GATEWAY, "host_adapter", detail)
            }
            RuntimeError::Internal(detail) => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", detail)
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": {
                    "code": self.code,
                    "detail": self.detail,
                }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{constant_time_equal, router, ServerState};
    use aelio_runtime::{ArtifactStatus, Runtime, RuntimeConfig, BASELINE_CONVERSATION_FLOW_ID};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn token_comparison_requires_exact_bytes() {
        assert!(constant_time_equal(
            b"abcdefghijklmnop",
            b"abcdefghijklmnop"
        ));
        assert!(!constant_time_equal(
            b"abcdefghijklmnop",
            b"abcdefghijklmnoq"
        ));
        assert!(!constant_time_equal(b"short", b"longer"));
    }

    fn runtime(path: &std::path::Path) -> Runtime {
        Runtime::open(RuntimeConfig {
            data_dir: path.into(),
            host_url: None,
            host_token: None,
            event_key_secret: [3; 32],
            queue_depth: 20,
        })
        .unwrap()
    }

    fn baseline_request(token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/v1/system/flows/conversation")
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder
            .body(Body::from(r#"{"tenant":"tenant-a"}"#))
            .unwrap()
    }

    #[tokio::test]
    async fn authenticated_baseline_install_is_gated_and_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = runtime(directory.path());
        let token = "0123456789abcdef0123456789abcdef";
        let app = router(ServerState::new(runtime.clone(), vec![token.into()], false).unwrap());
        assert_eq!(
            app.clone()
                .oneshot(baseline_request(Some(token)))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        assert_eq!(
            app.oneshot(baseline_request(Some(token)))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let record = runtime
            .artifact_repository()
            .unwrap()
            .get("tenant-a", BASELINE_CONVERSATION_FLOW_ID, 1)
            .unwrap()
            .unwrap();
        assert_eq!(record.status, ArtifactStatus::Canary);
    }

    #[tokio::test]
    async fn insecure_mode_cannot_approve_reviewed_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = runtime(directory.path());
        let app = router(ServerState::new(runtime.clone(), vec![], true).unwrap());
        assert_eq!(
            app.oneshot(baseline_request(None)).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let record = runtime
            .artifact_repository()
            .unwrap()
            .get("tenant-a", BASELINE_CONVERSATION_FLOW_ID, 1)
            .unwrap()
            .unwrap();
        assert_eq!(record.status, ArtifactStatus::Shadow);
    }

    #[tokio::test]
    async fn insecure_mode_cannot_create_prompt_admission_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = runtime(directory.path());
        let app = router(ServerState::new(runtime, vec![], true).unwrap());
        let request = Request::builder()
            .method("POST")
            .uri("/v1/mint/gate")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"tenant":"tenant-a","id":"missing","version":1,"cases":[]}"#,
            ))
            .unwrap();
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn authenticated_demand_is_deduplicated_and_bound_to_a_build() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = runtime(directory.path());
        let token = "0123456789abcdef0123456789abcdef";
        let app = router(ServerState::new(runtime.clone(), vec![token.into()], false).unwrap());
        let demand = serde_json::json!({
            "tenant":"tenant-a",
            "draft":{
                "normalized_need":"produce a typed answer",
                "inputs":[{"name":"turn","imprint":"aelio.turn.input@1","required":true,"sensitivity":"internal"}],
                "output":"aelio.turn.output@1",
                "allowed_effects":["pure"],
                "requester":"untrusted-body-value",
                "reason":"missing_capability",
                "evidence_refs":["turn:one"]
            }
        });
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/v1/capability-requests")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(demand.to_string()))
                .unwrap()
        };
        assert_eq!(
            app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::ACCEPTED
        );
        assert_eq!(
            app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::ACCEPTED
        );
        let observed = runtime.list_capability_requests("tenant-a", 10).unwrap();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].demand_count, 2);
        assert!(observed[0].draft.requester.starts_with("api-key-"));

        let examples: Vec<_> = (0..3)
            .map(|index| {
                serde_json::json!({
                    "inputs":{"turn":{"index":index}},
                    "output":{"ok":true},
                    "negative":index == 0,
                    "fixtures":[]
                })
            })
            .collect();
        let queue = serde_json::json!({
            "tenant":"tenant-a",
            "spec":{
                "name":"answer.generated",
                "description":"produce a typed answer",
                "inputs":[{"name":"turn","imprint":"aelio.turn.input@1","required":true,"sensitivity":"internal"}],
                "output":"aelio.turn.output@1",
                "budget":{"max_depth":4,"max_children":4,"max_llm_calls":4,"max_tokens":10000,"max_reactions":100,"max_wall_ms":60000},
                "scope":{"tenant":"tenant-a","registries":["answer.*"]},
                "policy":{"principal_grants":[],"allowed_effects":["pure"],"denied_effects":["write","external"]},
                "examples":examples
            }
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/capability-requests/{}/queue",
                        observed[0].request_id
                    ))
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(queue.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
}
