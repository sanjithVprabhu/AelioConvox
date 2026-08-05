//! Durable turn saga around the Rust hot loop.

use chrono::Utc;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::abilities::invoke::{sanitize_response, CapabilityHost, MockToolHost};
use crate::abilities::registry::SituationFilter;
use crate::blocks::flow::FlowInstance;
use crate::blocks::turn::{TurnResult, TurnTraceStep};
use crate::contract::AbilityContract;
use crate::provider::ProviderCall;
use crate::runtime::learning::{
    ColdProposal, ColdProposalState, DependencySnapshot, PromotedProcedureVersion, ProposalEvidence,
};
use crate::storage::{
    AelioStore, CompareSwap, LogicalTable, PutIfAbsent, RecordEnvelope, StoredRecord,
};
use crate::tenant::TenantDecl;
use crate::tenant::ToolSpec;
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use crate::World;

const TOOL_LEASE_MS: i64 = 30_000;

fn durable_turn_request_hash(request: &DurableTurnRequest, channel: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for value in [&request.user_id, &request.utterance, channel] {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    hex::encode(hasher.finalize())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EmbeddingSpaceBinding {
    space_id: String,
    dimension: usize,
}

fn bind_embedding_space(
    store: &mut AelioStore,
    tenant_id: &str,
    embedder: &dyn crate::embedding::Embedder,
    now_ms: i64,
) -> AelioResult<()> {
    const KEY: &str = "embedding-space-v1";
    let expected = EmbeddingSpaceBinding {
        space_id: embedder.space_id(),
        dimension: embedder.dimension(),
    };
    if let Some(existing) =
        store.get::<EmbeddingSpaceBinding>(tenant_id, LogicalTable::Migrations, KEY)?
    {
        if existing.envelope.value.space_id != expected.space_id
            || existing.envelope.value.dimension != expected.dimension
        {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                "configured embedder does not match the tenant's persisted vector space",
            ));
        }
        return Ok(());
    }

    // A legacy store with unlabelled vectors cannot be safely guessed into a model space even when
    // dimensions happen to match. It needs an explicit re-embedding migration.
    let mut legacy_vectors_present = false;
    for table in [
        LogicalTable::Procedures,
        LogicalTable::Documents,
        LogicalTable::DocumentChunks,
        LogicalTable::Memories,
    ] {
        if !store
            .list::<serde_json::Value>(tenant_id, table, None, 1)?
            .is_empty()
        {
            legacy_vectors_present = true;
            break;
        }
    }
    if legacy_vectors_present && !expected.space_id.starts_with("aelio.hash-v1:") {
        return Err(AelioError::new(
            ReasonCode::Conflict,
            "legacy persisted vectors have no embedding-space identity; re-embed them before semantic startup",
        ));
    }

    let inserted = store.put_if_absent(
        tenant_id,
        LogicalTable::Migrations,
        &RecordEnvelope {
            key: KEY.into(),
            kind: "embedding_space_binding".into(),
            status: "active".into(),
            owner: "runtime".into(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            expires_at_ms: None,
            value: expected.clone(),
        },
        None,
    )?;
    if matches!(inserted, PutIfAbsent::Existing { .. }) {
        let winner = store
            .get::<EmbeddingSpaceBinding>(tenant_id, LogicalTable::Migrations, KEY)?
            .ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Conflict,
                    "embedding-space binding raced and disappeared",
                )
            })?;
        if winner.envelope.value.space_id != expected.space_id
            || winner.envelope.value.dimension != expected.dimension
        {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                "another runtime bound this tenant to a different embedding space",
            ));
        }
    }
    Ok(())
}

fn provider_token_total(calls: &[ProviderCall]) -> u64 {
    calls.iter().fold(0_u64, |total, call| {
        let tokens = match call {
            ProviderCall::Llm {
                response: Some(response),
                ..
            } => response
                .input_tokens
                .unwrap_or_default()
                .saturating_add(response.output_tokens.unwrap_or_default()),
            _ => 0,
        };
        total.saturating_add(tokens)
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableTurnRequest {
    /// Caller-stable idempotency key for this inbound turn.
    pub turn_id: String,
    pub user_id: String,
    pub utterance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ExternalStateCommand {
    user_id: String,
    state_id: String,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DurableTurnValue {
    pub(crate) user_id: String,
    /// Binds an idempotency key to the original caller and payload without persisting plaintext.
    #[serde(default)]
    pub(crate) request_hash: String,
    pub(crate) result: Option<TurnResult>,
    #[serde(default)]
    pub(crate) error: Option<AelioError>,
}

/// One hot-loop step attempt, durable for audit and golden-trace diffs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableStepAttempt {
    pub turn_id: String,
    pub user_id: String,
    pub index: u32,
    pub name: String,
    pub detail: String,
}

/// One provider call (LLM/embedding) bracketed to a turn for cost + replay audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnProviderCallRecord {
    pub turn_id: String,
    pub user_id: String,
    pub call: ProviderCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TermSynonymEvidence {
    term: String,
    attribute: String,
    confirmer_hashes: std::collections::BTreeSet<String>,
    promoted: bool,
}

const TERM_SYNONYM_MIN_CONFIRMERS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableIdempotencyState {
    Processing,
    /// The caller's deadline elapsed after dispatch; the external effect may have committed.
    UnknownOutcome,
    Completed,
    Failed,
    ManualReview,
}

/// Durable lease and sanitized outcome for one generic executor tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableIdempotencyRecord {
    pub key: String,
    pub tool_id: String,
    pub tool_version: String,
    pub user_id: String,
    pub idempotent: bool,
    pub state: DurableIdempotencyState,
    pub attempts: u32,
    pub lease_owner: String,
    pub lease_expires_at_ms: Option<i64>,
    pub safe_result: Option<Value>,
    pub reason_code: Option<ReasonCode>,
    pub failure_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkDeliveryDispositionAudit {
    /// Stable non-secret correlation digest; the raw socket correlation is never persisted.
    pub correlation_hash: String,
    pub disposition: String,
    pub recorded_at_ms: i64,
}

struct DurableToolHost {
    tenant_id: String,
    owner: String,
    store: AelioStore,
    inner: Box<dyn CapabilityHost>,
}

impl DurableToolHost {
    fn persist_outcome(
        &mut self,
        row: &StoredRecord<DurableIdempotencyRecord>,
        state: DurableIdempotencyState,
        safe_result: Option<Value>,
        error: Option<&AelioError>,
        now: i64,
    ) -> AelioResult<()> {
        let mut value = row.envelope.value.clone();
        value.state = state;
        value.lease_expires_at_ms = None;
        value.safe_result = safe_result;
        value.reason_code = error.map(|item| item.code);
        value.failure_message = error.map(|item| redact_text(&item.message));
        let next = RecordEnvelope {
            key: row.envelope.key.clone(),
            kind: row.envelope.kind.clone(),
            status: idempotency_status(&value.state).into(),
            owner: self.owner.clone(),
            created_at_ms: row.envelope.created_at_ms,
            updated_at_ms: now,
            expires_at_ms: None,
            value,
        };
        match self.store.compare_swap(
            &self.tenant_id,
            LogicalTable::Idempotency,
            row.row_id,
            row.version,
            &next,
            None,
        )? {
            CompareSwap::Updated { .. } => Ok(()),
            CompareSwap::Conflict { .. } | CompareSwap::NotFound => Err(AelioError::new(
                ReasonCode::Conflict,
                "tool outcome lost its durable idempotency lease",
            )),
        }
    }
}

impl CapabilityHost for DurableToolHost {
    fn call_with_context(
        &mut self,
        tool: &ToolSpec,
        args: &IndexMap<String, Value>,
        idempotency_key: &str,
        user_id: &str,
        channel: &str,
    ) -> AelioResult<Value> {
        let now = Utc::now().timestamp_millis();
        let value = DurableIdempotencyRecord {
            key: idempotency_key.into(),
            tool_id: tool.id.clone(),
            tool_version: tool.version.clone(),
            user_id: user_id.into(),
            idempotent: tool.idempotent,
            state: DurableIdempotencyState::Processing,
            attempts: 1,
            lease_owner: self.owner.clone(),
            lease_expires_at_ms: Some(now.saturating_add(TOOL_LEASE_MS)),
            safe_result: None,
            reason_code: None,
            failure_message: None,
        };
        let pending = RecordEnvelope {
            key: idempotency_key.into(),
            kind: "tool_idempotency".into(),
            status: "processing".into(),
            owner: self.owner.clone(),
            created_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: value.lease_expires_at_ms,
            value,
        };
        let row = match self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Idempotency,
            &pending,
            None,
        )? {
            PutIfAbsent::Inserted { .. } => self
                .store
                .get(&self.tenant_id, LogicalTable::Idempotency, idempotency_key)?
                .ok_or_else(|| {
                    AelioError::new(ReasonCode::Internal, "idempotency lease was not readable")
                })?,
            PutIfAbsent::Existing { .. } => {
                let current: StoredRecord<DurableIdempotencyRecord> = self
                    .store
                    .get(&self.tenant_id, LogicalTable::Idempotency, idempotency_key)?
                    .ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::Internal,
                            "existing idempotency row was not readable",
                        )
                    })?;
                match current.envelope.value.state {
                    DurableIdempotencyState::Completed => {
                        return current.envelope.value.safe_result.ok_or_else(|| {
                            AelioError::new(
                                ReasonCode::Internal,
                                "completed idempotency row has no safe result",
                            )
                        });
                    }
                    DurableIdempotencyState::Failed => {
                        return Err(AelioError::new(
                            current
                                .envelope
                                .value
                                .reason_code
                                .unwrap_or(ReasonCode::ToolError),
                            current
                                .envelope
                                .value
                                .failure_message
                                .unwrap_or_else(|| "previous tool invocation failed".into()),
                        ));
                    }
                    DurableIdempotencyState::ManualReview => {
                        return Err(AelioError::new(
                            ReasonCode::NeedsEscalation,
                            "tool outcome requires manual review",
                        ));
                    }
                    DurableIdempotencyState::Processing
                    | DurableIdempotencyState::UnknownOutcome => {}
                }
                if current
                    .envelope
                    .value
                    .lease_expires_at_ms
                    .is_some_and(|expires| expires > now)
                {
                    return Err(AelioError::new(
                        ReasonCode::Conflict,
                        "tool invocation is already leased",
                    ));
                }
                if !tool.idempotent {
                    self.persist_outcome(
                        &current,
                        DurableIdempotencyState::ManualReview,
                        None,
                        None,
                        now,
                    )?;
                    return Err(AelioError::new(
                        ReasonCode::NeedsEscalation,
                        "non-idempotent tool has an unknown outcome after restart",
                    ));
                }
                let mut next = current.envelope.clone();
                next.owner = self.owner.clone();
                next.updated_at_ms = now;
                next.expires_at_ms = Some(now.saturating_add(TOOL_LEASE_MS));
                next.value.lease_owner = self.owner.clone();
                next.value.lease_expires_at_ms = next.expires_at_ms;
                next.value.attempts = next.value.attempts.saturating_add(1);
                match self.store.compare_swap(
                    &self.tenant_id,
                    LogicalTable::Idempotency,
                    current.row_id,
                    current.version,
                    &next,
                    None,
                )? {
                    CompareSwap::Updated { .. } => self
                        .store
                        .get(&self.tenant_id, LogicalTable::Idempotency, idempotency_key)?
                        .ok_or_else(|| {
                            AelioError::new(
                                ReasonCode::Internal,
                                "renewed idempotency lease was not readable",
                            )
                        })?,
                    CompareSwap::Conflict { .. } | CompareSwap::NotFound => {
                        return Err(AelioError::new(
                            ReasonCode::Conflict,
                            "tool idempotency lease was claimed concurrently",
                        ));
                    }
                }
            }
        };

        match self
            .inner
            .call_with_context(tool, args, idempotency_key, user_id, channel)
        {
            Ok(raw) => {
                let safe = sanitize_response(&raw, tool)?;
                self.persist_outcome(
                    &row,
                    DurableIdempotencyState::Completed,
                    Some(safe),
                    None,
                    Utc::now().timestamp_millis(),
                )?;
                Ok(raw)
            }
            Err(error) => {
                if error.code == ReasonCode::Timeout || is_dispatched_unknown_outcome(&error) {
                    let state = if tool.idempotent {
                        DurableIdempotencyState::UnknownOutcome
                    } else {
                        DurableIdempotencyState::ManualReview
                    };
                    self.persist_outcome(
                        &row,
                        state,
                        None,
                        Some(&error),
                        Utc::now().timestamp_millis(),
                    )?;
                    if !tool.idempotent {
                        return Err(
                            AelioError::new(
                                ReasonCode::NeedsEscalation,
                                "non-idempotent tool lost its result after dispatch; outcome is unknown",
                            )
                            .with_decision_trace(
                                error.decision_trace.as_deref().map(str::to_owned),
                            ),
                        );
                    }
                    return Err(error);
                }
                self.persist_outcome(
                    &row,
                    DurableIdempotencyState::Failed,
                    None,
                    Some(&error),
                    Utc::now().timestamp_millis(),
                )?;
                Err(error)
            }
        }
    }

    fn invocation_count(&self) -> Option<usize> {
        self.inner.invocation_count()
    }

    fn invocation_count_for(&self, tool_id: &str) -> Option<usize> {
        self.inner.invocation_count_for(tool_id)
    }

    fn take_decision_trace(&mut self) -> Option<String> {
        self.inner.take_decision_trace()
    }
}

fn is_dispatched_unknown_outcome(error: &AelioError) -> bool {
    error.code == ReasonCode::Internal
        && matches!(
            error.detail.as_ref(),
            Some(Value::Map(detail))
                if detail.get("outcome").and_then(Value::as_str) == Some("unknown_outcome")
        )
}

fn idempotency_status(state: &DurableIdempotencyState) -> &'static str {
    match state {
        DurableIdempotencyState::Processing => "processing",
        DurableIdempotencyState::UnknownOutcome => "unknown_outcome",
        DurableIdempotencyState::Completed => "completed",
        DurableIdempotencyState::Failed => "failed",
        DurableIdempotencyState::ManualReview => "manual_review",
    }
}

fn redact_text(text: &str) -> String {
    let email = regex::Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b")
        .expect("static email redaction regex");
    // Require non-alphanumeric boundaries so digit-heavy hashes, correlation ids and artifact
    // identities are never mistaken for phone numbers. Preserve the surrounding delimiters.
    let phone = regex::Regex::new(r"(^|[^A-Za-z0-9])(\+?\d(?:[\s().-]*\d){9,14})($|[^A-Za-z0-9])")
        .expect("static phone redaction regex");
    let otp = regex::Regex::new(r"\b\d{6}\b").expect("static OTP redaction regex");
    let redacted = email.replace_all(text, "[REDACTED:pii]");
    let redacted = phone.replace_all(&redacted, "$1[REDACTED:pii]$3");
    otp.replace_all(&redacted, "[REDACTED:secret]").into_owned()
}

fn explicit_memory_id(user_id: &str, fact: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{user_id}\u{1f}{fact}").as_bytes());
    format!("explicit.{}", hex::encode(&digest[..12]))
}

fn deferred_intent_id(user_id: &str, turn_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{user_id}\u{1f}{turn_id}").as_bytes());
    format!("open-loop.{}", hex::encode(&digest[..12]))
}

fn is_sensitive_reference(value: &serde_json::Value) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.contains_key("$aelio_sensitive_ref"))
}

fn redact_provider_call(call: &mut ProviderCall) {
    if let ProviderCall::Llm {
        redacted_prompt,
        response,
        ..
    } = call
    {
        *redacted_prompt = redact_text(redacted_prompt);
        if let Some(response) = response {
            response.content = redact_text(&response.content);
        }
    }
}

/// Owns the tenant world and its embedded Aelio DB repositories.
pub struct DurableRuntime {
    tenant_id: String,
    pub world: World,
    pub store: AelioStore,
    embedder: std::sync::Arc<dyn crate::embedding::Embedder>,
    exploration_policy: crate::runtime::learning::ExplorationPolicy,
}

impl DurableRuntime {
    pub fn new(world: World, store: AelioStore) -> AelioResult<Self> {
        let embedder = std::sync::Arc::new(crate::embedding::HashEmbedder::new(
            store.embedding_dimension(),
        )?);
        Self::new_with_embedder(world, store, embedder)
    }

    pub fn embedding_space_id(&self) -> String {
        self.embedder.space_id()
    }

    pub fn has_semantic_embedder(&self) -> bool {
        self.embedder.supports_semantic_equivalence()
    }

    /// Install an external execution boundary without bypassing durable idempotency. Server
    /// adapters must use this instead of replacing `world.tool_host` directly. Production installs
    /// a [`CapabilityHost`] only — never a raw-dispatch legacy ToolHost on the hot path.
    pub fn install_tool_host(&mut self, inner: Box<dyn CapabilityHost>) {
        let now = Utc::now().timestamp_millis();
        self.world.tool_host = Box::new(DurableToolHost {
            tenant_id: self.tenant_id.clone(),
            owner: format!("runtime:{}:{now}:external-host", std::process::id()),
            store: self.store.clone(),
            inner,
        });
    }

    /// Persist one terminal reverse-SDK delivery disposition without retaining the raw
    /// correlation id or result payload. Repeated identical acknowledgements coalesce, while the
    /// accepted/duplicate/late distinctions remain independently auditable.
    pub fn record_sdk_delivery_disposition(
        &mut self,
        invocation_id: &str,
        disposition: &str,
    ) -> AelioResult<PutIfAbsent> {
        if invocation_id.trim().is_empty()
            || invocation_id.len() > 256
            || !matches!(disposition, "accepted" | "duplicate" | "late")
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "SDK delivery audit requires a bounded correlation and closed disposition",
            ));
        }
        use sha2::{Digest, Sha256};
        let correlation_hash = hex::encode(Sha256::digest(invocation_id.as_bytes()));
        let now = Utc::now().timestamp_millis();
        self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::SdkDeliveryEvents,
            &envelope(
                format!("{correlation_hash}:{disposition}"),
                "sdk_delivery_result",
                disposition,
                "aelio-wire",
                now,
                SdkDeliveryDispositionAudit {
                    correlation_hash,
                    disposition: disposition.into(),
                    recorded_at_ms: now,
                },
            ),
            None,
        )
    }

    pub fn list_sdk_delivery_dispositions(
        &self,
        limit: usize,
    ) -> AelioResult<Vec<StoredRecord<SdkDeliveryDispositionAudit>>> {
        if limit == 0 || limit > 1_000 {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "SDK delivery audit limit must be within 1..=1000",
            ));
        }
        self.store.list(
            &self.tenant_id,
            LogicalTable::SdkDeliveryEvents,
            None,
            limit,
        )
    }

    /// Bounded operational view of durable tool outcomes. The API layer must hash subject and
    /// idempotency identities and must never expose `safe_result` or failure free text.
    pub fn list_tool_outcomes(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> AelioResult<Vec<StoredRecord<DurableIdempotencyRecord>>> {
        if limit == 0 || limit > 1_000 {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "tool outcome audit limit must be within 1..=1000",
            ));
        }
        if status.is_some_and(|status| {
            !matches!(
                status,
                "processing" | "unknown_outcome" | "completed" | "failed" | "manual_review"
            )
        }) {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "tool outcome audit status is not in the closed state vocabulary",
            ));
        }
        self.store
            .list(&self.tenant_id, LogicalTable::Idempotency, status, limit)
    }

    pub fn list_open_loops(
        &self,
        limit: usize,
    ) -> AelioResult<Vec<StoredRecord<crate::memory::Memory>>> {
        if limit == 0 || limit > 1_000 {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "open-loop audit limit must be within 1..=1000",
            ));
        }
        Ok(self
            .store
            .list::<crate::memory::Memory>(
                &self.tenant_id,
                LogicalTable::Memories,
                Some("active"),
                1_000,
            )?
            .into_iter()
            .filter(|row| row.envelope.value.kind == crate::memory::MemoryKind::OpenLoop)
            .take(limit)
            .collect())
    }

    /// Construct production recall and memory writes over one shared embedding space.
    pub fn new_with_embedder(
        mut world: World,
        mut store: AelioStore,
        embedder: std::sync::Arc<dyn crate::embedding::Embedder>,
    ) -> AelioResult<Self> {
        if embedder.dimension() != store.embedding_dimension() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "runtime embedder dimension does not match the tenant store",
            ));
        }
        let tenant_id = world.tenant.tenant_id.clone();
        let now = Utc::now().timestamp_millis();
        store.migrate_tenant(&tenant_id, now)?;
        bind_embedding_space(&mut store, &tenant_id, embedder.as_ref(), now)?;
        if let Some(active) =
            store.get::<TenantDecl>(&tenant_id, LogicalTable::Catalogs, "active")?
        {
            if active.envelope.status == "active" {
                validate_catalog(&active.envelope.value)?;
                world.replace_tenant(active.envelope.value)?;
            }
        }
        let inner = std::mem::replace(&mut world.tool_host, Box::new(MockToolHost::default()));
        world.tool_host = Box::new(DurableToolHost {
            tenant_id: tenant_id.clone(),
            owner: format!("runtime:{}:{now}", std::process::id()),
            store: store.clone(),
            inner,
        });
        world.turn_recall = Box::new(crate::runtime::turn_recall::StoreTurnRecall::with_embedder(
            &tenant_id,
            store.clone(),
            embedder.clone(),
        )?);
        world.set_shared_embedder(embedder.clone());
        world.inline_learning_enabled = false;
        world.durable_memory_enabled = true;
        let promoted_proposals: Vec<StoredRecord<ColdProposal>> = store.list(
            &tenant_id,
            LogicalTable::Proposals,
            Some("promoted"),
            10_000,
        )?;
        if !promoted_proposals.is_empty() {
            let mut cold_loop = crate::runtime::learning::LearningColdLoop::with_embedder(
                &tenant_id,
                world.tenant.mode,
                store.clone(),
                embedder.clone(),
            )?;
            for proposal in promoted_proposals {
                cold_loop.promote(
                    &proposal.envelope.key,
                    &crate::runtime::learning::PromotionGate::default(),
                    now,
                )?;
            }
        }
        let current_tools = world
            .tenant
            .tools
            .iter()
            .map(|tool| (tool.id.clone(), tool.version.clone()))
            .collect();
        crate::runtime::learning::LearningColdLoop::new(
            &tenant_id,
            world.tenant.mode,
            store.clone(),
        )
        .invalidate_dependencies(&current_tools, &world.registry, now)?;
        world.user_state.clear();
        world.user_flows.clear();
        let exploration_policy = store
            .get::<crate::runtime::learning::ExplorationPolicy>(
                &tenant_id,
                LogicalTable::Explorations,
                "policy",
            )?
            .filter(|row| row.envelope.status == "active")
            .map(|row| row.envelope.value)
            .unwrap_or_default();
        exploration_policy.validate()?;
        let mut runtime = Self {
            tenant_id,
            world,
            store,
            embedder,
            exploration_policy,
        };
        runtime.refresh_procedures()?;
        Ok(runtime)
    }

    pub fn configure_exploration(
        &mut self,
        policy: crate::runtime::learning::ExplorationPolicy,
    ) -> AelioResult<()> {
        policy.validate()?;
        let now = Utc::now().timestamp_millis();
        self.upsert(
            LogicalTable::Explorations,
            envelope(
                "policy",
                "exploration_policy",
                "active",
                "operator",
                now,
                policy.clone(),
            ),
        )?;
        self.exploration_policy = policy;
        Ok(())
    }

    /// Apply a lifecycle state supplied by the tenant's authenticated SDK.
    ///
    /// The command id is caller-stable. Replays are harmless; reusing it for different content
    /// fails closed. This is the only supported external lifecycle write path.
    pub fn set_user_state(
        &mut self,
        command_id: &str,
        user_id: &str,
        state_id: &str,
        reason: Option<&str>,
    ) -> AelioResult<bool> {
        if command_id.trim().is_empty()
            || command_id.len() > 256
            || user_id.trim().is_empty()
            || user_id.len() > 256
            || state_id.trim().is_empty()
            || state_id.len() > 256
            || reason.is_some_and(|value| value.len() > 2_048)
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "state command fields are required and must stay within bounds",
            ));
        }
        if !self
            .world
            .tenant
            .states
            .iter()
            .any(|state| state.id == state_id)
        {
            return Err(AelioError::new(
                ReasonCode::NotFound,
                format!("state `{state_id}` is not declared by the active tenant catalog"),
            ));
        }
        let command = ExternalStateCommand {
            user_id: user_id.into(),
            state_id: state_id.into(),
            reason: reason.map(redact_text),
        };
        let audit_key = format!("state_command:{command_id}");
        if let Some(existing) = self.store.get::<ExternalStateCommand>(
            &self.tenant_id,
            LogicalTable::Audit,
            &audit_key,
        )? {
            if existing.envelope.value != command {
                return Err(AelioError::new(
                    ReasonCode::Conflict,
                    "state command id was reused for different content",
                ));
            }
            self.world
                .user_state
                .insert(user_id.into(), state_id.into());
            return Ok(true);
        }

        let now = Utc::now().timestamp_millis();
        self.upsert(
            LogicalTable::States,
            envelope(
                user_id,
                "state",
                "active",
                "tenant_sdk",
                now,
                state_id.to_owned(),
            ),
        )?;
        self.world
            .user_state
            .insert(user_id.into(), state_id.into());
        match self.store.put_if_absent(
            &self.tenant_id,
            LogicalTable::Audit,
            &envelope(
                audit_key,
                "state_command",
                "applied",
                "tenant_sdk",
                now,
                command,
            ),
            None,
        )? {
            PutIfAbsent::Inserted { .. } => Ok(false),
            PutIfAbsent::Existing { .. } => Err(AelioError::new(
                ReasonCode::Conflict,
                "state command was applied concurrently",
            )),
        }
    }

    pub fn list_learning_proposals(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> AelioResult<Vec<StoredRecord<ColdProposal>>> {
        if limit == 0 || limit > 1_000 {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "proposal list limit must be within 1..=1000",
            ));
        }
        self.store
            .list(&self.tenant_id, LogicalTable::Proposals, status, limit)
    }

    pub fn approve_learning_proposal(
        &mut self,
        proposal_id: &str,
        approved_by: &str,
    ) -> AelioResult<PutIfAbsent> {
        crate::runtime::learning::LearningColdLoop::new(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
        )
        .approve(proposal_id, approved_by, Utc::now().timestamp_millis())
    }

    pub fn promote_learning_proposal(
        &mut self,
        proposal_id: &str,
        gate: &crate::runtime::learning::PromotionGate,
    ) -> AelioResult<crate::runtime::learning::PromotionResult> {
        let result = crate::runtime::learning::LearningColdLoop::with_embedder(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
            self.embedder.clone(),
        )?
        .promote(proposal_id, gate, Utc::now().timestamp_millis())?;
        self.refresh_procedures()?;
        Ok(result)
    }

    pub fn list_promoted_procedures(
        &self,
        limit: usize,
    ) -> AelioResult<Vec<StoredRecord<PromotedProcedureVersion>>> {
        if limit == 0 || limit > 1_000 {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "procedure list limit must be within 1..=1000",
            ));
        }
        self.store.list(
            &self.tenant_id,
            LogicalTable::Procedures,
            Some("promoted"),
            limit,
        )
    }

    pub fn is_procedure_version_valid(&self, key: &str) -> AelioResult<bool> {
        crate::runtime::learning::LearningColdLoop::new(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
        )
        .is_valid(key)
    }

    pub fn suspend_learning_procedure(
        &mut self,
        procedure_id: &str,
        reason: &str,
    ) -> AelioResult<usize> {
        if procedure_id.trim().is_empty() || reason.trim().is_empty() || reason.len() > 1_000 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "procedure id and bounded suspension reason are required",
            ));
        }
        let suspended = crate::runtime::learning::LearningColdLoop::new(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
        )
        .suspend_procedure(procedure_id, reason, Utc::now().timestamp_millis())?;
        self.refresh_procedures()?;
        Ok(suspended)
    }

    pub fn propose_candidate_flow(
        &mut self,
        flow: crate::tenant::FlowSpec,
        source_procedure_versions: Vec<String>,
    ) -> AelioResult<PutIfAbsent> {
        let protected: Vec<_> = self
            .world
            .tenant
            .flows
            .iter()
            .filter(|existing| !existing.learnable)
            .collect();
        if protected
            .iter()
            .any(|existing| flow_trigger_overlap(existing, &flow))
        {
            return Err(AelioError::new(
                ReasonCode::PolicyDenied,
                "candidate flow trigger overlaps a protected authored rail",
            ));
        }
        let mut source_dependencies = std::collections::BTreeMap::new();
        for source_key in &source_procedure_versions {
            let source: StoredRecord<crate::runtime::learning::PromotedProcedureVersion> = self
                .store
                .get(&self.tenant_id, LogicalTable::Procedures, source_key)?
                .ok_or_else(|| {
                    AelioError::new(ReasonCode::NotFound, "source procedure was not found")
                })?;
            for (tool_id, version) in source.envelope.value.dependencies.tool_versions {
                source_dependencies.insert((tool_id, version), ());
            }
        }
        for admissible in flow.steps.iter().flat_map(|step| &step.admissible) {
            let matching_tools = self
                .world
                .tenant
                .tools
                .iter()
                .filter(|tool| tool.capability_tags.iter().any(|tag| tag == admissible))
                .collect::<Vec<_>>();
            if matching_tools.len() != 1 {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    "candidate flow admissible must resolve to exactly one current tool",
                ));
            }
            let tool = matching_tools[0];
            if !source_dependencies.contains_key(&(tool.id.clone(), tool.version.clone())) {
                return Err(AelioError::new(
                    ReasonCode::Denied,
                    "candidate flow tool/version is not proven by its source procedures",
                ));
            }
        }
        let mut validation_catalog = self.world.tenant.clone();
        if let Some(index) = validation_catalog
            .flows
            .iter()
            .position(|existing| existing.id == flow.id)
        {
            validation_catalog.flows[index] = flow.clone();
        } else {
            validation_catalog.flows.push(flow.clone());
        }
        validate_catalog(&validation_catalog)?;
        let protected_ids = protected
            .iter()
            .map(|flow| flow.id.clone())
            .collect::<Vec<_>>();
        crate::runtime::learning::LearningColdLoop::new(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
        )
        .propose_candidate_flow(
            flow,
            source_procedure_versions,
            &protected_ids,
            Utc::now().timestamp_millis(),
        )
    }

    pub fn review_candidate_flow(
        &mut self,
        key: &str,
        approved: bool,
        reviewed_by: &str,
        reason: Option<String>,
    ) -> AelioResult<()> {
        crate::runtime::learning::LearningColdLoop::new(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
        )
        .review_candidate_flow(
            key,
            approved,
            reviewed_by,
            reason,
            Utc::now().timestamp_millis(),
        )
    }

    pub fn list_candidate_flows(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> AelioResult<Vec<StoredRecord<crate::runtime::learning::FlowCandidate>>> {
        if limit > 1_000 {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "candidate flow list limit exceeds 1000",
            ));
        }
        self.store
            .list(&self.tenant_id, LogicalTable::FlowCandidates, status, limit)
    }

    /// Activate only an explicitly approved candidate. Protected rails and overlapping trigger
    /// surfaces cannot be replaced by learning, even when `allow_replace` is true.
    pub fn activate_candidate_flow(&mut self, key: &str, allow_replace: bool) -> AelioResult<()> {
        self.activate_candidate_flow_with_artifact(key, allow_replace, None)
    }

    /// Activate a reviewed learned flow and, in unified mode, atomically publish the exact
    /// executable artifact that owns its continuation/effect authority. Missing or invalid
    /// materialization is checked before the candidate leaves `Approved`.
    pub fn activate_candidate_flow_with_artifact(
        &mut self,
        key: &str,
        allow_replace: bool,
        artifact: Option<crate::adaptive::ArtifactPinV1>,
    ) -> AelioResult<()> {
        if !self.world.legacy_flow_execution_enabled && artifact.is_none() {
            return Err(AelioError::new(
                ReasonCode::GateNotMet,
                "unified flow activation requires an admitted runtime artifact",
            ));
        }
        if let Some(pin) = &artifact {
            pin.validate()?;
            self.world
                .adaptive_artifact_host
                .verify_executable_pin(&self.tenant_id, pin)?;
        }

        let now = Utc::now().timestamp_millis();
        let mut candidate = loop {
            let row: StoredRecord<crate::runtime::learning::FlowCandidate> = self
                .store
                .get(&self.tenant_id, LogicalTable::FlowCandidates, key)?
                .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "candidate flow not found"))?;
            match row.envelope.value.state {
                crate::runtime::learning::FlowCandidateState::Active => return Ok(()),
                crate::runtime::learning::FlowCandidateState::Approved => {
                    let mut next = row.envelope;
                    next.value.state = crate::runtime::learning::FlowCandidateState::Activating;
                    next.status = "activating".into();
                    next.updated_at_ms = now;
                    match self.store.compare_swap(
                        &self.tenant_id,
                        LogicalTable::FlowCandidates,
                        row.row_id,
                        row.version,
                        &next,
                        None,
                    )? {
                        CompareSwap::Updated { .. } => break next.value,
                        CompareSwap::Conflict { .. } | CompareSwap::NotFound => continue,
                    }
                }
                crate::runtime::learning::FlowCandidateState::Activating => {
                    break row.envelope.value;
                }
                crate::runtime::learning::FlowCandidateState::PendingReview
                | crate::runtime::learning::FlowCandidateState::Rejected => {
                    return Err(AelioError::new(
                        ReasonCode::PolicyDenied,
                        "candidate flow must be explicitly approved before activation",
                    ));
                }
            }
        };
        let cold_loop = crate::runtime::learning::LearningColdLoop::new(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
        );
        for procedure_key in &candidate.source_procedure_versions {
            if !cold_loop.is_valid(procedure_key)? {
                self.reject_activating_candidate(
                    key,
                    "source procedure was invalidated after review",
                    now,
                )?;
                return Err(AelioError::new(
                    ReasonCode::GateNotMet,
                    "candidate flow source was invalidated after review",
                ));
            }
        }
        for existing in &self.world.tenant.flows {
            if !existing.learnable && flow_trigger_overlap(existing, &candidate.flow) {
                self.reject_activating_candidate(
                    key,
                    "candidate overlaps a protected authored rail",
                    now,
                )?;
                return Err(AelioError::new(
                    ReasonCode::PolicyDenied,
                    "approved candidate overlaps a protected authored rail",
                ));
            }
        }
        let mut tenant = self.world.tenant.clone();
        if let Some(index) = tenant
            .flows
            .iter()
            .position(|existing| existing.id == candidate.flow.id)
        {
            let existing = &tenant.flows[index];
            if !allow_replace || !existing.learnable {
                self.reject_activating_candidate(
                    key,
                    "replacement was not explicitly allowed or the target is protected",
                    now,
                )?;
                return Err(AelioError::new(
                    ReasonCode::PolicyDenied,
                    "candidate replacement requires an explicitly replaceable authored flow",
                ));
            }
            tenant.flows[index] = candidate.flow.clone();
        } else {
            tenant.flows.push(candidate.flow.clone());
        }
        if let Some(pin) = artifact {
            tenant.flow_artifacts.insert(candidate.flow.id.clone(), pin);
        }
        self.register_catalog(tenant)?;
        for _ in 0..8 {
            let row: StoredRecord<crate::runtime::learning::FlowCandidate> = self
                .store
                .get(&self.tenant_id, LogicalTable::FlowCandidates, key)?
                .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "candidate flow not found"))?;
            if row.envelope.value.state == crate::runtime::learning::FlowCandidateState::Active {
                return Ok(());
            }
            if row.envelope.value.state != crate::runtime::learning::FlowCandidateState::Activating
            {
                return Err(AelioError::new(
                    ReasonCode::Conflict,
                    "candidate flow activation state changed unexpectedly",
                ));
            }
            let mut next = row.envelope;
            candidate.state = crate::runtime::learning::FlowCandidateState::Active;
            next.value = candidate.clone();
            next.status = "active".into();
            next.updated_at_ms = now;
            match self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::FlowCandidates,
                row.row_id,
                row.version,
                &next,
                None,
            )? {
                CompareSwap::Updated { .. } => return Ok(()),
                CompareSwap::Conflict { .. } => continue,
                CompareSwap::NotFound => {
                    return Err(AelioError::new(
                        ReasonCode::NotFound,
                        "candidate flow disappeared during activation",
                    ));
                }
            }
        }
        Err(AelioError::new(
            ReasonCode::Conflict,
            "candidate flow activation CAS contention exceeded",
        ))
    }

    fn reject_activating_candidate(
        &mut self,
        key: &str,
        reason: &str,
        now_ms: i64,
    ) -> AelioResult<()> {
        for _ in 0..8 {
            let row: StoredRecord<crate::runtime::learning::FlowCandidate> = self
                .store
                .get(&self.tenant_id, LogicalTable::FlowCandidates, key)?
                .ok_or_else(|| AelioError::new(ReasonCode::NotFound, "candidate flow not found"))?;
            if row.envelope.value.state == crate::runtime::learning::FlowCandidateState::Rejected {
                return Ok(());
            }
            if row.envelope.value.state != crate::runtime::learning::FlowCandidateState::Activating
            {
                return Err(AelioError::new(
                    ReasonCode::Conflict,
                    "candidate activation state changed during terminal rejection",
                ));
            }
            let mut next = row.envelope;
            next.value.state = crate::runtime::learning::FlowCandidateState::Rejected;
            next.value.review_reason = Some(reason.into());
            next.status = "rejected".into();
            next.owner = "activation_gate".into();
            next.updated_at_ms = now_ms;
            match self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::FlowCandidates,
                row.row_id,
                row.version,
                &next,
                None,
            )? {
                CompareSwap::Updated { .. } => return Ok(()),
                CompareSwap::Conflict { .. } => continue,
                CompareSwap::NotFound => {
                    return Err(AelioError::new(
                        ReasonCode::NotFound,
                        "candidate flow disappeared during terminal rejection",
                    ));
                }
            }
        }
        Err(AelioError::new(
            ReasonCode::Conflict,
            "candidate terminal rejection CAS contention exceeded",
        ))
    }

    /// Run one turn as an idempotent saga. A completed turn is replayed from durable storage;
    /// an in-progress duplicate is rejected so two workers cannot execute the same effects.
    pub fn run_turn(&mut self, request: DurableTurnRequest) -> AelioResult<TurnResult> {
        self.run_turn_on_channel(request, "unknown")
    }

    pub fn run_turn_on_channel(
        &mut self,
        request: DurableTurnRequest,
        channel: &str,
    ) -> AelioResult<TurnResult> {
        if request.turn_id.trim().is_empty()
            || request.user_id.trim().is_empty()
            || request.turn_id.len() > 256
            || request.user_id.len() > 256
            || request.utterance.trim().is_empty()
            || request.utterance.len() > 32_768
            || channel.trim().is_empty()
            || channel.len() > 128
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "turn_id, user_id, and utterance are required and must stay within bounds",
            ));
        }
        self.refresh_procedures()?;
        let now = Utc::now().timestamp_millis();
        let request_hash = durable_turn_request_hash(&request, channel);
        let pending = envelope(
            request.turn_id.clone(),
            "turn",
            "processing",
            "runtime",
            now,
            DurableTurnValue {
                user_id: request.user_id.clone(),
                request_hash: request_hash.clone(),
                result: None,
                error: None,
            },
        );
        let (turn_row, turn_version) =
            match self
                .store
                .put_if_absent(&self.tenant_id, LogicalTable::Turns, &pending, None)?
            {
                PutIfAbsent::Inserted { row_id, version } => (row_id, version),
                PutIfAbsent::Existing { .. } => {
                    let existing: StoredRecord<DurableTurnValue> = self
                        .store
                        .get(&self.tenant_id, LogicalTable::Turns, &request.turn_id)?
                        .ok_or_else(|| {
                            AelioError::new(
                                ReasonCode::Conflict,
                                "turn exists but could not be loaded",
                            )
                        })?;
                    if existing.envelope.value.user_id != request.user_id
                        || (!existing.envelope.value.request_hash.is_empty()
                            && existing.envelope.value.request_hash != request_hash)
                    {
                        return Err(AelioError::new(
                            ReasonCode::Conflict,
                            "turn idempotency key was reused for a different request",
                        ));
                    }
                    if existing.envelope.status == "completed" {
                        return existing.envelope.value.result.ok_or_else(|| {
                            AelioError::new(
                                ReasonCode::Internal,
                                "completed turn has no durable result",
                            )
                        });
                    }
                    if existing.envelope.status == "failed" {
                        return Err(existing.envelope.value.error.unwrap_or_else(|| {
                            AelioError::new(
                                ReasonCode::Internal,
                                "previous turn attempt failed without a durable error",
                            )
                        }));
                    }
                    return Err(AelioError::new(
                        ReasonCode::Conflict,
                        "turn is already being processed",
                    ));
                }
            };

        let outcome = (|| -> AelioResult<TurnResult> {
            self.hydrate_user(&request.user_id)?;
            let pending_term_confirmation = self
                .world
                .legacy_flow_execution_enabled
                .then(|| {
                    self.world
                        .user_flows
                        .get(&request.user_id)
                        .filter(|flow| flow.flow_id == "__aelio.term_confirmation")
                        .and_then(|flow| {
                            Some((
                                flow.slots.get("term")?.as_str()?.to_owned(),
                                flow.slots.get("attribute")?.as_str()?.to_owned(),
                            ))
                        })
                })
                .flatten();
            let state_before = self
                .world
                .user_state
                .get(&request.user_id)
                .cloned()
                .unwrap_or_else(|| "unauthenticated".into());
            let provider_calls_before = self.world.provider_calls().len();
            let turn_started = std::time::Instant::now();
            let runtime_flow_was_active = self
                .world
                .adaptive_artifact_host
                .has_active_subject(&self.tenant_id, &request.user_id)?;
            let mut result = self.world.run_turn_on_channel_with_id(
                &request.user_id,
                &request.utterance,
                channel,
                &request.turn_id,
            );
            if let Some(fact) = crate::memory::explicit_fact(&request.utterance) {
                let mut policy = crate::policy::PolicyCtx {
                    state: self.world.user_state.get(&request.user_id).cloned(),
                    tenant: Some(self.tenant_id.clone()),
                    capability: Some("memory.write".into()),
                    action_risk: crate::policy::PolicyActionRisk::MemoryWrite,
                    ..Default::default()
                };
                policy
                    .slots
                    .insert("user_id".into(), Value::str(&request.user_id));
                match crate::policy::require_allow(&self.world.tenant.policies, &policy)
                    .and_then(|()| self.remember_explicit_fact(&request, fact, now))
                {
                    Ok(()) => {
                        result.steps.push(TurnTraceStep {
                            name: "Memory.Store".into(),
                            detail: "stored explicit user-scoped factual memory".into(),
                        });
                        result.reply = crate::abilities::express::template(
                            "memory_stored",
                            "I'll remember that.",
                            &IndexMap::new(),
                        );
                    }
                    Err(error) => {
                        result.steps.push(TurnTraceStep {
                            name: "Memory.Store".into(),
                            detail: format!("rejected: {:?}", error.code),
                        });
                        result.reply = crate::abilities::express::apologize(
                            "I couldn't safely store that memory",
                            "Please check the tenant memory policy.",
                        );
                    }
                }
            } else if let Some(fact) = crate::memory::explicit_forget_fact(&request.utterance) {
                let policy = crate::policy::PolicyCtx {
                    state: self.world.user_state.get(&request.user_id).cloned(),
                    tenant: Some(self.tenant_id.clone()),
                    capability: Some("memory.delete".into()),
                    action_risk: crate::policy::PolicyActionRisk::MemoryDelete,
                    slots: indexmap::indexmap! {
                        "user_id".into() => Value::str(&request.user_id),
                    },
                    ..Default::default()
                };
                match crate::policy::require_allow(&self.world.tenant.policies, &policy)
                    .and_then(|()| self.forget_explicit_fact(&request.user_id, fact, now))
                {
                    Ok(()) => {
                        result.steps.push(TurnTraceStep {
                            name: "Memory.Forget".into(),
                            detail: "retired exact user-scoped factual memory".into(),
                        });
                        result.reply = crate::abilities::express::template(
                            "memory_forgotten",
                            "I've forgotten that.",
                            &IndexMap::new(),
                        );
                    }
                    Err(error) => {
                        result.steps.push(TurnTraceStep {
                            name: "Memory.Forget".into(),
                            detail: format!("rejected: {:?}", error.code),
                        });
                        result.reply = crate::abilities::express::apologize(
                            "I couldn't forget that exact memory",
                            "Repeat the fact exactly or check the tenant memory policy.",
                        );
                    }
                }
            }
            if result
                .steps
                .iter()
                .any(|step| step.name == "FlowGate.Defer")
            {
                self.remember_deferred_intent(&request, now)?;
                result.steps.push(TurnTraceStep {
                    name: "Memory.OpenLoop".into(),
                    detail: "persisted deferred user intent; active runtime continuation unchanged"
                        .into(),
                });
            }
            if runtime_flow_was_active
                && !self
                    .world
                    .adaptive_artifact_host
                    .has_active_subject(&self.tenant_id, &request.user_id)?
            {
                if let Some(deferred) =
                    self.mark_next_deferred_intent_ready(&request.user_id, now)?
                {
                    result.steps.push(TurnTraceStep {
                        name: "Memory.OpenLoopReady".into(),
                        detail:
                            "original runtime flow completed; oldest deferred intent returned to the user"
                                .into(),
                    });
                    result.reply.text = format!(
                        "{} I also kept your deferred request: {}",
                        result.reply.text.trim_end(),
                        deferred.text
                    );
                    result.reply.frame = None;
                }
            }
            if result
                .steps
                .iter()
                .any(|step| step.name == "TermConfirm" && step.detail.starts_with("accepted"))
            {
                if let Some((term, attribute)) = pending_term_confirmation {
                    let (confirmations, promoted) =
                        self.record_term_confirmation(&term, &attribute, &request.user_id, now)?;
                    result.steps.push(TurnTraceStep {
                        name: "TermLearn".into(),
                        detail: if promoted {
                            format!(
                                "promoted tenant synonym {term} -> {attribute} after {confirmations} distinct confirmations"
                            )
                        } else {
                            format!(
                                "recorded {confirmations}/{TERM_SYNONYM_MIN_CONFIRMERS} distinct confirmations for {term} -> {attribute}"
                            )
                        },
                    });
                }
            }
            if let Some(step) = self.maybe_run_shadow_exploration(&request, &result, now) {
                result.steps.push(step);
            }
            self.persist_user(&request.user_id, &result, now)?;
            self.persist_steps(&request.turn_id, &request.user_id, &result.steps, now)?;
            self.persist_provider_calls(
                &request.turn_id,
                &request.user_id,
                provider_calls_before,
                now,
            )?;
            if let Some(proposal_id) = &result.proposal_id {
                if let Some(proposal) = self.world.proposals.get(proposal_id).cloned() {
                    let tool_dependency_ids = crate::abilities::learn::path_tool_dependencies(
                        &self.world.registry,
                        &proposal.path,
                    )?;
                    let tool_versions: std::collections::BTreeMap<String, String> =
                        tool_dependency_ids
                            .iter()
                            .filter_map(|tool_id| {
                                self.world
                                    .registry
                                    .tools
                                    .get(tool_id)
                                    .map(|tool| (tool_id.clone(), tool.version.clone()))
                            })
                            .collect();
                    if tool_versions.len() != tool_dependency_ids.len() {
                        return Err(AelioError::new(
                            ReasonCode::Validation,
                            "proposal path contains an unresolved tool dependency",
                        ));
                    }
                    let prompt_hash = crate::abilities::learn::path_prompt_dependency_fingerprint(
                        &self.world.registry,
                        &proposal.path,
                    );
                    let required_evidence = crate::abilities::learn::path_required_evidence(
                        &self.world.registry,
                        &proposal.path,
                    )?;
                    let used_capabilities = crate::abilities::learn::path_capabilities(
                        &self.world.registry,
                        &proposal.path,
                    )?;
                    let mut produced_evidence = crate::abilities::learn::path_produced_evidence(
                        &self.world.registry,
                        &proposal.path,
                    )?;
                    let situation_filter = proposal
                        .sigma
                        .as_ref()
                        .map(|sigma| SituationFilter {
                            state: Some(sigma.state.clone()),
                            intent_class: Some(sigma.intent_class.clone()),
                            required_slots: required_evidence,
                            capability_tags: used_capabilities,
                        })
                        .unwrap_or_else(|| SituationFilter {
                            state: Some(state_before),
                            intent_class: (result.depth == crate::types::Depth::Shallow)
                                .then(|| "greeting".into()),
                            ..Default::default()
                        });
                    let mut contract = AbilityContract::pure(proposal.id.clone());
                    contract.effectful = proposal.effectful;
                    contract.tool_deps = tool_dependency_ids;
                    if let Some(intent) = situation_filter.intent_class.clone() {
                        produced_evidence.push(intent);
                    }
                    produced_evidence.sort();
                    produced_evidence.dedup();
                    contract.postconditions = produced_evidence
                        .into_iter()
                        .map(|path| crate::contract::Predicate::Present { path })
                        .collect();
                    let durable = ColdProposal {
                        id: proposal.id.clone(),
                        situation_hash: proposal.situation_hash,
                        situation_filter,
                        path: proposal.path,
                        contract,
                        effectful: proposal.effectful,
                        dependencies: DependencySnapshot {
                            tool_versions,
                            prompt_hash,
                        },
                        evidence: ProposalEvidence::default(),
                        state: ColdProposalState::Accumulating,
                        promoted_version: None,
                    };
                    let _ = self.store.put_if_absent(
                        &self.tenant_id,
                        LogicalTable::Proposals,
                        &envelope(
                            proposal_id.clone(),
                            "proposal",
                            "accumulating",
                            "runtime",
                            now,
                            durable,
                        ),
                        None,
                    )?;
                }
            }

            let has_invoke_error = result.steps.iter().any(|step| step.name == "Invoke.Error");
            if let Some(proposal_id) = &result.proposal_id {
                // A registry may contain an authored/imported immutable procedure which has no
                // cold-loop proposal row. It is executable and observable, but it must not be
                // fed into proposal accumulation under an ID that does not exist. Runtime-earned
                // paths were persisted just above, so absence here specifically means "external
                // procedure", not a reason to fail an otherwise successful user turn.
                let durable_proposal = self
                    .store
                    .get::<crate::runtime::learning::ColdProposal>(
                        &self.tenant_id,
                        LogicalTable::Proposals,
                        proposal_id,
                    )?
                    .is_some();
                if !durable_proposal || (result.suspended && !has_invoke_error) {
                    // Missing user input is not a behavioral failure and must not poison learning.
                } else {
                    let kind = if result.steps.iter().any(|step| step.name == "Invoke.Error") {
                        crate::runtime::learning::BehavioralSignalKind::ToolError
                    } else if result.new_state.is_some() {
                        crate::runtime::learning::BehavioralSignalKind::FlowTerminal
                    } else if result.steps.iter().any(|step| step.name == "Invoke.Call") {
                        crate::runtime::learning::BehavioralSignalKind::ToolSuccess
                    } else if !result.suspended
                        && result.reply.via != crate::abilities::express::ExpressVia::Apologize
                    {
                        crate::runtime::learning::BehavioralSignalKind::TurnCompleted
                    } else {
                        crate::runtime::learning::BehavioralSignalKind::Abandonment
                    };
                    let mut cold_loop = crate::runtime::learning::LearningColdLoop::with_embedder(
                        &self.tenant_id,
                        self.world.tenant.mode,
                        self.store.clone(),
                        self.embedder.clone(),
                    )?;
                    let latency_ms =
                        u64::try_from(turn_started.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let token_cost =
                        provider_token_total(&self.world.provider_calls()[provider_calls_before..]);
                    cold_loop.record_signal(crate::runtime::learning::BehavioralOutcomeSignal {
                        id: format!("outcome:{}", request.turn_id),
                        turn_id: request.turn_id.clone(),
                        proposal_id: Some(proposal_id.clone()),
                        kind,
                        step_ids: result.steps.iter().map(|step| step.name.clone()).collect(),
                        failed_step: result
                            .steps
                            .iter()
                            .find(|step| {
                                step.name == "Invoke.Error" || step.detail.contains("failed")
                            })
                            .map(|step| step.name.clone()),
                        latency_ms,
                        token_cost,
                        call_cost_microunits: 0,
                        observed_at_ms: now,
                    })?;
                    if has_invoke_error
                        && self.world.registry.procedures.get(proposal_id).is_some_and(
                            |procedure| {
                                procedure.status
                                    == crate::abilities::registry::ProcedureStatus::Promoted
                            },
                        )
                    {
                        cold_loop.suspend_procedure(
                            proposal_id,
                            "promoted procedure produced a tool error",
                            now,
                        )?;
                        if let Some(procedure) = self.world.registry.procedures.get_mut(proposal_id)
                        {
                            procedure.status =
                                crate::abilities::registry::ProcedureStatus::Suspended;
                        }
                    }
                    match cold_loop.promote(
                        proposal_id,
                        &crate::runtime::learning::PromotionGate::default(),
                        now,
                    ) {
                        Ok(_) => {
                            self.refresh_procedures()?;
                        }
                        Err(error) if error.code == ReasonCode::GateNotMet => {}
                        Err(error) => return Err(error),
                    }
                }
            }

            let completed = envelope(
                request.turn_id.clone(),
                "turn",
                "completed",
                "runtime",
                now,
                DurableTurnValue {
                    user_id: request.user_id.clone(),
                    request_hash: request_hash.clone(),
                    result: Some(self.redact_turn_result(&result)),
                    error: None,
                },
            );
            match self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::Turns,
                turn_row,
                turn_version,
                &completed,
                None,
            )? {
                CompareSwap::Updated { .. } => {
                    if crate::decision_log::enabled() {
                        eprintln!(
                            "{}",
                            crate::decision_log::render_durable_commit(
                                &request.turn_id,
                                &request.user_id,
                            )
                        );
                    }
                    Ok(result)
                }
                CompareSwap::Conflict { .. } | CompareSwap::NotFound => Err(AelioError::new(
                    ReasonCode::Conflict,
                    "turn completion lost its ownership lease",
                )),
            }
        })();
        if let Err(error) = &outcome {
            let failed = envelope(
                request.turn_id.clone(),
                "turn",
                "failed",
                "runtime",
                now,
                DurableTurnValue {
                    user_id: request.user_id.clone(),
                    request_hash,
                    result: None,
                    error: Some(AelioError {
                        code: error.code,
                        message: redact_text(&error.message),
                        detail: None,
                        decision_trace: error.decision_trace.clone(),
                    }),
                },
            );
            let _ = self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::Turns,
                turn_row,
                turn_version,
                &failed,
                None,
            );
        }
        outcome
    }

    /// Refresh the active tier lookup from immutable promoted durable versions.
    pub fn refresh_procedures(&mut self) -> AelioResult<usize> {
        let procedures: Vec<StoredRecord<PromotedProcedureVersion>> = self.store.list(
            &self.tenant_id,
            LogicalTable::Procedures,
            Some("promoted"),
            10_000,
        )?;
        let mut best = std::collections::BTreeMap::<String, PromotedProcedureVersion>::new();
        let cold_loop = crate::runtime::learning::LearningColdLoop::new(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
        );
        for procedure in procedures {
            let immutable_key = procedure.envelope.key;
            let value = procedure.envelope.value;
            if value.spec.tenant_id != self.tenant_id {
                return Err(AelioError::new(
                    ReasonCode::Denied,
                    "durable procedure belongs to another tenant",
                ));
            }
            if !cold_loop.is_valid(&immutable_key)? {
                if let Some(current) = self.world.registry.procedures.get_mut(&value.spec.id) {
                    if current.version == value.spec.version {
                        current.status = crate::abilities::registry::ProcedureStatus::Suspended;
                    }
                }
                continue;
            }
            crate::abilities::learn::typecheck(&self.world.registry, &value.spec.path)?;
            match best.get(&value.procedure_id) {
                Some(current) if !promoted_version_is_better(&value, current) => {}
                _ => {
                    best.insert(value.procedure_id.clone(), value);
                }
            }
        }
        let mut loaded = 0;
        for procedure in best.into_values() {
            let spec = procedure.spec;
            if self
                .world
                .registry
                .procedures
                .get(&spec.id)
                .is_some_and(|current| current.version == spec.version)
            {
                continue;
            }
            self.world.registry.register_procedure(spec);
            loaded += 1;
        }
        self.refresh_procedure_artifact_bindings()?;
        self.refresh_flow_artifact_bindings()?;
        Ok(loaded)
    }

    /// Durably bind a legacy learned procedure identity to one exact unified runtime artifact.
    /// Bindings are immutable and live outside the learned path version so artifact admission can
    /// be rolled out independently without rewriting historical learning evidence.
    pub fn bind_procedure_artifact(
        &mut self,
        procedure_id: &str,
        artifact: crate::adaptive::ArtifactPinV1,
        actor: &str,
    ) -> AelioResult<()> {
        if actor.trim().is_empty() || actor.len() > 256 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "artifact binding actor is required and bounded",
            ));
        }
        artifact.validate()?;
        if !self.world.registry.procedures.contains_key(procedure_id) {
            return Err(AelioError::new(
                ReasonCode::NotFound,
                "cannot bind an artifact to an unknown promoted procedure",
            ));
        }
        let key = procedure_artifact_binding_key(procedure_id);
        let record = envelope(
            key.clone(),
            "procedure_artifact_binding",
            "active",
            actor,
            Utc::now().timestamp_millis(),
            artifact.clone(),
        );
        match self
            .store
            .put_if_absent(&self.tenant_id, LogicalTable::Audit, &record, None)?
        {
            PutIfAbsent::Inserted { .. } => {}
            PutIfAbsent::Existing { .. } => {
                let existing: StoredRecord<crate::adaptive::ArtifactPinV1> = self
                    .store
                    .get(&self.tenant_id, LogicalTable::Audit, &key)?
                    .ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::Conflict,
                            "artifact binding exists but could not be loaded",
                        )
                    })?;
                if existing.envelope.value != artifact {
                    return Err(AelioError::new(
                        ReasonCode::Conflict,
                        "procedure already has a different immutable artifact binding",
                    ));
                }
            }
        }
        self.world
            .registry
            .bind_procedure_artifact(procedure_id, artifact)
    }

    fn refresh_procedure_artifact_bindings(&mut self) -> AelioResult<()> {
        let procedure_ids = self
            .world
            .registry
            .procedures
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for procedure_id in procedure_ids {
            let key = procedure_artifact_binding_key(&procedure_id);
            let binding: Option<StoredRecord<crate::adaptive::ArtifactPinV1>> =
                self.store.get(&self.tenant_id, LogicalTable::Audit, &key)?;
            if let Some(binding) = binding {
                if binding.envelope.status != "active" {
                    return Err(AelioError::new(
                        ReasonCode::Conflict,
                        "procedure artifact binding has an invalid durable status",
                    ));
                }
                self.world
                    .registry
                    .bind_procedure_artifact(&procedure_id, binding.envelope.value)?;
            }
        }
        Ok(())
    }

    pub fn bind_flow_artifact(
        &mut self,
        flow_id: &str,
        artifact: crate::adaptive::ArtifactPinV1,
        actor: &str,
    ) -> AelioResult<()> {
        if actor.trim().is_empty() || actor.len() > 256 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "artifact binding actor is required and bounded",
            ));
        }
        artifact.validate()?;
        if !self.world.registry.flows.contains_key(flow_id) {
            return Err(AelioError::new(
                ReasonCode::NotFound,
                "cannot bind an artifact to an unknown authored flow",
            ));
        }
        let key = flow_artifact_binding_key(flow_id);
        let record = envelope(
            key.clone(),
            "flow_artifact_binding",
            "active",
            actor,
            Utc::now().timestamp_millis(),
            artifact.clone(),
        );
        match self
            .store
            .put_if_absent(&self.tenant_id, LogicalTable::Audit, &record, None)?
        {
            PutIfAbsent::Inserted { .. } => {}
            PutIfAbsent::Existing { .. } => {
                let existing: StoredRecord<crate::adaptive::ArtifactPinV1> = self
                    .store
                    .get(&self.tenant_id, LogicalTable::Audit, &key)?
                    .ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::Conflict,
                            "flow artifact binding exists but could not be loaded",
                        )
                    })?;
                if existing.envelope.value != artifact {
                    return Err(AelioError::new(
                        ReasonCode::Conflict,
                        "authored flow already has a different immutable artifact binding",
                    ));
                }
            }
        }
        self.world.registry.bind_flow_artifact(flow_id, artifact)
    }

    fn refresh_flow_artifact_bindings(&mut self) -> AelioResult<()> {
        let flow_ids = self
            .world
            .registry
            .flows
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for flow_id in flow_ids {
            let key = flow_artifact_binding_key(&flow_id);
            let binding: Option<StoredRecord<crate::adaptive::ArtifactPinV1>> =
                self.store.get(&self.tenant_id, LogicalTable::Audit, &key)?;
            if let Some(binding) = binding {
                if binding.envelope.status != "active" {
                    return Err(AelioError::new(
                        ReasonCode::Conflict,
                        "flow artifact binding has an invalid durable status",
                    ));
                }
                self.world
                    .registry
                    .bind_flow_artifact(&flow_id, binding.envelope.value)?;
            }
        }
        Ok(())
    }

    fn maybe_run_shadow_exploration(
        &mut self,
        request: &DurableTurnRequest,
        primary: &TurnResult,
        now_ms: i64,
    ) -> Option<TurnTraceStep> {
        if !self.exploration_policy.enabled
            || !matches!(
                primary.tier,
                Some(crate::types::LookupTier::Tier0 | crate::types::LookupTier::Tier1)
            )
            || primary.suspended
            || primary.active_flow.is_some()
            || primary.new_state.is_some()
        {
            return None;
        }
        let (Some(primary_id), Some(situation_hash)) = (
            primary.proposal_id.as_deref(),
            primary.situation_hash.as_deref(),
        ) else {
            return None;
        };
        let mut cold_loop = match crate::runtime::learning::LearningColdLoop::with_embedder(
            &self.tenant_id,
            self.world.tenant.mode,
            self.store.clone(),
            self.embedder.clone(),
        ) {
            Ok(loop_) => loop_,
            Err(error) => {
                return Some(TurnTraceStep {
                    name: "Explore.Shadow".into(),
                    detail: format!("skipped: embedder setup failed with {:?}", error.code),
                });
            }
        };
        let candidate = match cold_loop.runner_up(
            situation_hash,
            primary_id,
            self.exploration_policy.max_candidate_steps,
        ) {
            Ok(Some(candidate)) => candidate,
            Ok(None) => return None,
            Err(error) => {
                return Some(TurnTraceStep {
                    name: "Explore.Shadow".into(),
                    detail: format!("skipped: runner-up lookup failed with {:?}", error.code),
                });
            }
        };
        let current_tools: std::collections::BTreeMap<_, _> = self
            .world
            .tenant
            .tools
            .iter()
            .map(|tool| (tool.id.clone(), tool.version.clone()))
            .collect();
        if candidate
            .dependencies
            .tool_versions
            .iter()
            .any(|(tool, version)| current_tools.get(tool) != Some(version))
            || crate::abilities::learn::path_is_effectful(&self.world.registry, &candidate.path)
        {
            return Some(TurnTraceStep {
                name: "Explore.Shadow".into(),
                detail: format!(
                    "skipped unsafe or stale candidate {}@{}",
                    candidate.procedure_id, candidate.version
                ),
            });
        }
        match cold_loop.claim_live_exploration(&request.turn_id, &self.exploration_policy, now_ms) {
            Ok(true) => {}
            Ok(false) => return None,
            Err(error) => {
                return Some(TurnTraceStep {
                    name: "Explore.Shadow".into(),
                    detail: format!("skipped: budget claim failed with {:?}", error.code),
                });
            }
        }
        let started = std::time::Instant::now();
        let outcome =
            self.world
                .run_shadow_path(&request.user_id, &request.utterance, &candidate.path);
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let primary_success = !primary.suspended
            && primary.reply.via != crate::abilities::express::ExpressVia::Apologize
            && !primary.steps.iter().any(|step| step.name == "Invoke.Error");
        let (candidate_success, evidence_hash, invoked_tools, failure_code) = match &outcome {
            Ok(shadow) => (
                true,
                Some(shadow.evidence_hash.clone()),
                shadow.invoked_tools.clone(),
                None,
            ),
            Err(error) => (false, None, Vec::new(), Some(error.code)),
        };
        let candidate_key = format!("{}@{}", candidate.procedure_id, candidate.version);
        let comparison = crate::runtime::learning::ExplorationComparison {
            turn_id: request.turn_id.clone(),
            primary_procedure: primary_id.into(),
            candidate_procedure: candidate_key.clone(),
            primary_success,
            candidate_success,
            candidate_evidence_hash: evidence_hash,
            candidate_latency_ms: latency_ms,
            invoked_tools: invoked_tools.clone(),
            failure_code,
            observed_at_ms: now_ms,
        };
        if let Err(error) = cold_loop.record_exploration_comparison(comparison) {
            return Some(TurnTraceStep {
                name: "Explore.Shadow".into(),
                detail: format!(
                    "ran {candidate_key} but comparison persistence failed: {:?}",
                    error.code
                ),
            });
        }
        let signal_kind = if candidate_success {
            if invoked_tools.is_empty() {
                crate::runtime::learning::BehavioralSignalKind::TurnCompleted
            } else {
                crate::runtime::learning::BehavioralSignalKind::ToolSuccess
            }
        } else {
            crate::runtime::learning::BehavioralSignalKind::ToolError
        };
        if let Err(error) =
            cold_loop.record_signal(crate::runtime::learning::BehavioralOutcomeSignal {
                id: format!("exploration:{}:{candidate_key}", request.turn_id),
                turn_id: request.turn_id.clone(),
                proposal_id: Some(candidate.procedure_id.clone()),
                kind: signal_kind,
                step_ids: candidate
                    .path
                    .steps
                    .iter()
                    .map(|step| step.ability_id.clone())
                    .collect(),
                failed_step: (!candidate_success).then(|| "shadow_execution".into()),
                latency_ms,
                token_cost: 0,
                call_cost_microunits: 0,
                observed_at_ms: now_ms,
            })
        {
            return Some(TurnTraceStep {
                name: "Explore.Shadow".into(),
                detail: format!(
                    "ran {candidate_key} but signal persistence failed: {:?}",
                    error.code
                ),
            });
        }
        match cold_loop.promote(
            &candidate.procedure_id,
            &crate::runtime::learning::PromotionGate::default(),
            now_ms,
        ) {
            Ok(crate::runtime::learning::PromotionResult::Promoted { .. }) => {
                if let Err(error) = self.refresh_procedures() {
                    return Some(TurnTraceStep {
                        name: "Explore.Shadow".into(),
                        detail: format!(
                            "recorded {candidate_key} but refresh failed: {:?}",
                            error.code
                        ),
                    });
                }
            }
            Ok(crate::runtime::learning::PromotionResult::AlreadyPromoted { .. }) => {}
            Err(error) if error.code == ReasonCode::GateNotMet => {}
            Err(error) => {
                return Some(TurnTraceStep {
                    name: "Explore.Shadow".into(),
                    detail: format!(
                        "recorded {candidate_key} but re-promotion failed: {:?}",
                        error.code
                    ),
                });
            }
        }
        Some(TurnTraceStep {
            name: "Explore.Shadow".into(),
            detail: if candidate_success {
                format!(
                    "compared primary={primary_id} candidate={candidate_key} read_only=true tools={} latency_ms={latency_ms}",
                    invoked_tools.len()
                )
            } else {
                format!("candidate={candidate_key} failed safely with {failure_code:?}")
            },
        })
    }

    fn record_term_confirmation(
        &mut self,
        term: &str,
        attribute: &str,
        user_id: &str,
        now_ms: i64,
    ) -> AelioResult<(usize, bool)> {
        if term.is_empty()
            || term.chars().count() > 128
            || !self
                .world
                .tenant
                .attributes
                .iter()
                .any(|candidate| candidate.name == attribute && candidate.sortable)
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "term synonym must target a declared sortable attribute",
            ));
        }
        use sha2::{Digest, Sha256};
        let key_hash =
            Sha256::digest(format!("{attribute}\u{1f}{}", term.to_ascii_lowercase()).as_bytes());
        let key = format!("term_synonym:{}", hex::encode(&key_hash[..12]));
        let user_digest = Sha256::digest(user_id.as_bytes());
        let user_hash = hex::encode(&user_digest[..12]);
        for _ in 0..8 {
            let existing: Option<StoredRecord<TermSynonymEvidence>> =
                self.store.get(&self.tenant_id, LogicalTable::Audit, &key)?;
            let (count, newly_promoted) = if let Some(row) = existing {
                let mut next = row.envelope;
                next.value.confirmer_hashes.insert(user_hash.clone());
                let count = next.value.confirmer_hashes.len();
                let newly_promoted = !next.value.promoted && count >= TERM_SYNONYM_MIN_CONFIRMERS;
                next.value.promoted |= newly_promoted;
                next.status = if next.value.promoted {
                    "promoted".into()
                } else {
                    "accumulating".into()
                };
                next.updated_at_ms = now_ms;
                match self.store.compare_swap(
                    &self.tenant_id,
                    LogicalTable::Audit,
                    row.row_id,
                    row.version,
                    &next,
                    None,
                )? {
                    CompareSwap::Updated { .. } => (count, newly_promoted),
                    CompareSwap::Conflict { .. } | CompareSwap::NotFound => continue,
                }
            } else {
                let mut confirmer_hashes = std::collections::BTreeSet::new();
                confirmer_hashes.insert(user_hash.clone());
                let evidence = TermSynonymEvidence {
                    term: term.to_ascii_lowercase(),
                    attribute: attribute.into(),
                    confirmer_hashes,
                    promoted: TERM_SYNONYM_MIN_CONFIRMERS <= 1,
                };
                match self.store.put_if_absent(
                    &self.tenant_id,
                    LogicalTable::Audit,
                    &envelope(
                        key.clone(),
                        "term_synonym_evidence",
                        if evidence.promoted {
                            "promoted"
                        } else {
                            "accumulating"
                        },
                        "term_learning",
                        now_ms,
                        evidence,
                    ),
                    None,
                )? {
                    PutIfAbsent::Inserted { .. } => (1, TERM_SYNONYM_MIN_CONFIRMERS <= 1),
                    PutIfAbsent::Existing { .. } => continue,
                }
            };
            if newly_promoted {
                let mut tenant = self.world.tenant.clone();
                let declared = tenant
                    .attributes
                    .iter_mut()
                    .find(|candidate| candidate.name == attribute)
                    .ok_or_else(|| {
                        AelioError::new(ReasonCode::NotFound, "confirmed attribute disappeared")
                    })?;
                if !declared
                    .learned
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(term))
                {
                    declared.learned.push(term.to_ascii_lowercase());
                    self.register_catalog(tenant)?;
                    self.refresh_procedures()?;
                }
            }
            return Ok((count, newly_promoted));
        }
        Err(AelioError::new(
            ReasonCode::Conflict,
            "term confirmation evidence changed too many times",
        ))
    }

    fn remember_explicit_fact(
        &mut self,
        request: &DurableTurnRequest,
        fact: &str,
        now_ms: i64,
    ) -> AelioResult<()> {
        if fact.chars().count() > 2_000
            || regex::Regex::new(r"\b\d{6}\b")
                .expect("static secret regex")
                .is_match(fact)
            || fact.to_ascii_lowercase().contains("bearer ")
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "memory is empty, too large, or resembles a secret",
            ));
        }
        let memory = crate::memory::Memory {
            id: explicit_memory_id(&request.user_id, fact),
            kind: crate::memory::MemoryKind::Factual,
            text: fact.into(),
            provenance: crate::memory::MemoryProvenance {
                source_kind: "explicit_user_instruction".into(),
                source_id: request.turn_id.clone(),
                observed_at_ms: now_ms,
                actor: request.user_id.clone(),
            },
            confidence: 1.0,
            valid_time: crate::memory::ValidTime {
                from_ms: now_ms,
                to_ms: None,
            },
            expires_at_ms: None,
            decay: crate::memory::DecayPolicy {
                half_life_ms: None,
                floor: 1.0,
            },
            entity_id: None,
            open_loop_state: None,
            procedure_version: None,
        };
        let mut memories = crate::memory::MemoryStore::new(&self.tenant_id, self.store.clone());
        memories.remember(&request.user_id, memory, self.embedder.as_ref(), now_ms)?;
        Ok(())
    }

    fn remember_deferred_intent(
        &mut self,
        request: &DurableTurnRequest,
        now_ms: i64,
    ) -> AelioResult<()> {
        let memory = crate::memory::Memory {
            id: deferred_intent_id(&request.user_id, &request.turn_id),
            kind: crate::memory::MemoryKind::OpenLoop,
            text: redact_text(&request.utterance),
            provenance: crate::memory::MemoryProvenance {
                source_kind: "flow_gate_deferred_intent".into(),
                source_id: request.turn_id.clone(),
                observed_at_ms: now_ms,
                actor: request.user_id.clone(),
            },
            confidence: 1.0,
            valid_time: crate::memory::ValidTime {
                from_ms: now_ms,
                to_ms: None,
            },
            expires_at_ms: None,
            decay: crate::memory::DecayPolicy {
                half_life_ms: None,
                floor: 1.0,
            },
            entity_id: None,
            open_loop_state: Some("deferred".into()),
            procedure_version: None,
        };
        crate::memory::MemoryStore::new(&self.tenant_id, self.store.clone()).remember(
            &request.user_id,
            memory,
            self.embedder.as_ref(),
            now_ms,
        )?;
        Ok(())
    }

    fn mark_next_deferred_intent_ready(
        &mut self,
        user_id: &str,
        now_ms: i64,
    ) -> AelioResult<Option<crate::memory::Memory>> {
        let mut candidates: Vec<StoredRecord<crate::memory::Memory>> = self
            .store
            .list::<crate::memory::Memory>(
                &self.tenant_id,
                LogicalTable::Memories,
                Some("active"),
                1_000,
            )?
            .into_iter()
            .filter(|row| {
                row.envelope.owner == user_id
                    && row.envelope.value.kind == crate::memory::MemoryKind::OpenLoop
                    && row.envelope.value.open_loop_state.as_deref() == Some("deferred")
            })
            .collect();
        candidates.sort_by_key(|row| row.envelope.value.provenance.observed_at_ms);
        let Some(row) = candidates.into_iter().next() else {
            return Ok(None);
        };
        let mut next = row.envelope;
        next.value.open_loop_state = Some("ready".into());
        next.updated_at_ms = now_ms;
        let memory = next.value.clone();
        match self.store.compare_swap(
            &self.tenant_id,
            LogicalTable::Memories,
            row.row_id,
            row.version,
            &next,
            None,
        )? {
            CompareSwap::Updated { .. } => Ok(Some(memory)),
            CompareSwap::Conflict { .. } => Err(AelioError::new(
                ReasonCode::Conflict,
                "deferred intent changed while returning to it",
            )),
            CompareSwap::NotFound => Err(AelioError::new(
                ReasonCode::NotFound,
                "deferred intent disappeared while returning to it",
            )),
        }
    }

    fn forget_explicit_fact(&mut self, user_id: &str, fact: &str, now_ms: i64) -> AelioResult<()> {
        let key = explicit_memory_id(user_id, fact);
        for _ in 0..8 {
            let Some(row) = self.store.get::<crate::memory::Memory>(
                &self.tenant_id,
                LogicalTable::Memories,
                &key,
            )?
            else {
                return Err(AelioError::new(
                    ReasonCode::NotFound,
                    "exact memory was not found",
                ));
            };
            if row.envelope.owner != user_id {
                return Err(AelioError::new(
                    ReasonCode::Denied,
                    "memory belongs to another user",
                ));
            }
            if row.envelope.status == "forgotten" {
                return Ok(());
            }
            let mut next = row.envelope;
            next.status = "forgotten".into();
            next.updated_at_ms = now_ms;
            next.value.valid_time.to_ms = Some(now_ms);
            match self.store.compare_swap(
                &self.tenant_id,
                LogicalTable::Memories,
                row.row_id,
                row.version,
                &next,
                None,
            )? {
                CompareSwap::Updated { .. } => return Ok(()),
                CompareSwap::Conflict { .. } => continue,
                CompareSwap::NotFound => {
                    return Err(AelioError::new(ReasonCode::NotFound, "memory disappeared"));
                }
            }
        }
        Err(AelioError::new(
            ReasonCode::Conflict,
            "memory changed too many times while being forgotten",
        ))
    }

    /// Load every durable step attempt for a turn (ordered by index).
    pub fn list_steps(&self, turn_id: &str) -> AelioResult<Vec<StoredRecord<DurableStepAttempt>>> {
        let rows: Vec<StoredRecord<DurableStepAttempt>> =
            self.store
                .list(&self.tenant_id, LogicalTable::StepAttempts, None, 256)?;
        let mut matched: Vec<_> = rows
            .into_iter()
            .filter(|row| row.envelope.value.turn_id == turn_id)
            .collect();
        matched.sort_by_key(|row| row.envelope.value.index);
        Ok(matched)
    }

    /// Load provider call records for a turn.
    pub fn list_calls(
        &self,
        turn_id: &str,
    ) -> AelioResult<Vec<StoredRecord<TurnProviderCallRecord>>> {
        let rows: Vec<StoredRecord<TurnProviderCallRecord>> =
            self.store
                .list(&self.tenant_id, LogicalTable::CallRecords, None, 256)?;
        let mut matched: Vec<_> = rows
            .into_iter()
            .filter(|row| row.envelope.value.turn_id == turn_id)
            .collect();
        matched.sort_by(|a, b| a.envelope.key.cmp(&b.envelope.key));
        Ok(matched)
    }

    /// Load a completed turn result from durable storage.
    pub fn get_turn(&self, turn_id: &str) -> AelioResult<Option<TurnResult>> {
        let row: Option<StoredRecord<DurableTurnValue>> =
            self.store
                .get(&self.tenant_id, LogicalTable::Turns, turn_id)?;
        Ok(row.and_then(|r| r.envelope.value.result))
    }

    fn persist_steps(
        &mut self,
        turn_id: &str,
        user_id: &str,
        steps: &[TurnTraceStep],
        now: i64,
    ) -> AelioResult<()> {
        for (index, step) in steps.iter().enumerate() {
            let key = format!("{turn_id}:step:{index:03}");
            let value = DurableStepAttempt {
                turn_id: turn_id.into(),
                user_id: user_id.into(),
                index: index as u32,
                name: step.name.clone(),
                detail: redact_text(&step.detail),
            };
            let _ = self.store.put_if_absent(
                &self.tenant_id,
                LogicalTable::StepAttempts,
                &envelope(key, "step_attempt", "recorded", "runtime", now, value),
                None,
            )?;
        }
        Ok(())
    }

    fn persist_provider_calls(
        &mut self,
        turn_id: &str,
        user_id: &str,
        from_index: usize,
        now: i64,
    ) -> AelioResult<()> {
        let calls = self.world.provider_calls()[from_index..].to_vec();
        for (i, mut call) in calls.into_iter().enumerate() {
            redact_provider_call(&mut call);
            let key = format!("{turn_id}:call:{i:03}");
            let value = TurnProviderCallRecord {
                turn_id: turn_id.into(),
                user_id: user_id.into(),
                call,
            };
            let _ = self.store.put_if_absent(
                &self.tenant_id,
                LogicalTable::CallRecords,
                &envelope(key, "provider_call", "recorded", "runtime", now, value),
                None,
            )?;
        }
        Ok(())
    }

    /// Validate, durably publish, and activate a complete tenant catalog snapshot.
    pub fn register_catalog(&mut self, tenant: TenantDecl) -> AelioResult<()> {
        if tenant.tenant_id != self.tenant_id {
            return Err(AelioError::new(
                ReasonCode::Denied,
                "catalog tenant does not match this runtime",
            ));
        }
        validate_catalog(&tenant)?;
        let now = Utc::now().timestamp_millis();
        let current_tools = tenant
            .tools
            .iter()
            .map(|tool| (tool.id.clone(), tool.version.clone()))
            .collect();
        crate::runtime::learning::LearningColdLoop::new(
            &self.tenant_id,
            tenant.mode,
            self.store.clone(),
        )
        .invalidate_dependencies(&current_tools, &self.world.registry, now)?;
        self.upsert(
            LogicalTable::Catalogs,
            envelope(
                "active",
                "tenant_catalog",
                "active",
                "registration",
                now,
                tenant.clone(),
            ),
        )?;
        self.world.replace_tenant(tenant)?;
        self.refresh_procedures()?;
        Ok(())
    }

    fn hydrate_user(&mut self, user_id: &str) -> AelioResult<()> {
        let state: Option<StoredRecord<String>> =
            self.store
                .get(&self.tenant_id, LogicalTable::States, user_id)?;
        if let Some(state) = state {
            self.world
                .user_state
                .insert(user_id.into(), state.envelope.value);
        }
        if self.world.legacy_flow_execution_enabled && !self.world.user_flows.contains_key(user_id)
        {
            let flow: Option<StoredRecord<Option<FlowInstance>>> =
                self.store
                    .get(&self.tenant_id, LogicalTable::FlowInstances, user_id)?;
            match flow.and_then(|record| record.envelope.value) {
                Some(mut instance) => {
                    instance
                        .slots
                        .retain(|_, value| !is_sensitive_reference(value));
                    self.world.user_flows.insert(user_id.into(), instance);
                }
                None => {
                    self.world.user_flows.shift_remove(user_id);
                }
            }
        }
        if !self.world.user_harness.contains_key(user_id) {
            let harness: Option<crate::storage::StoredRecord<crate::harness::HarnessSession>> =
                self.store
                    .get(&self.tenant_id, LogicalTable::HarnessSessions, user_id)?;
            if let Some(record) = harness {
                self.world
                    .user_harness
                    .insert(user_id.into(), record.envelope.value);
            }
        }
        Ok(())
    }

    fn persist_user(&mut self, user_id: &str, result: &TurnResult, now: i64) -> AelioResult<()> {
        if let Some(state) = self.world.user_state.get(user_id).cloned() {
            self.upsert(
                LogicalTable::States,
                envelope(user_id, "state", "active", "runtime", now, state),
            )?;
        }
        if let Some(harness) = self.world.user_harness.get(user_id).cloned() {
            let status = if harness.is_empty() {
                "empty"
            } else if harness.waiting_child().is_some() {
                "waiting"
            } else {
                "active"
            };
            self.upsert(
                LogicalTable::HarnessSessions,
                envelope(user_id, "harness_session", status, "runtime", now, harness),
            )?;
        }
        if self.world.legacy_flow_execution_enabled {
            let flow_status = if result.active_flow.is_some() {
                "active"
            } else {
                "closed"
            };
            self.upsert(
                LogicalTable::FlowInstances,
                envelope(
                    user_id,
                    "flow_instance",
                    flow_status,
                    "runtime",
                    now,
                    self.redact_flow(result.active_flow.clone()),
                ),
            )?;
        }
        Ok(())
    }

    fn redact_turn_result(&self, result: &TurnResult) -> TurnResult {
        let mut safe = result.clone();
        safe.reply.text = redact_text(&safe.reply.text);
        for step in &mut safe.steps {
            step.detail = redact_text(&step.detail);
        }
        safe.active_flow = self.redact_flow(safe.active_flow);
        safe
    }

    fn redact_flow(&self, flow: Option<FlowInstance>) -> Option<FlowInstance> {
        let mut flow = flow?;
        for key in ["clauses", "__aelio_plan_clauses"] {
            if let Some(clauses) = flow
                .slots
                .get_mut(key)
                .and_then(serde_json::Value::as_array_mut)
            {
                for clause in clauses {
                    if let Some(text) = clause.as_str() {
                        *clause = serde_json::Value::String(redact_text(text));
                    }
                }
            }
        }
        for tool in &self.world.tenant.tools {
            for param in &tool.params {
                if matches!(param.sensitivity, crate::types::Sensitivity::None) {
                    continue;
                }
                if flow.slots.contains_key(&param.name) {
                    let kind = match param.sensitivity {
                        crate::types::Sensitivity::Pii => "pii",
                        crate::types::Sensitivity::Secret => "secret",
                        crate::types::Sensitivity::None => unreachable!(),
                    };
                    flow.slots.insert(
                        param.name.clone(),
                        serde_json::json!({
                            "$aelio_sensitive_ref": {
                                "kind": kind,
                                "resolvable": false
                            }
                        }),
                    );
                }
            }
        }
        Some(flow)
    }

    fn upsert<T>(&mut self, logical: LogicalTable, next: RecordEnvelope<T>) -> AelioResult<()>
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        let current: Option<StoredRecord<T>> =
            self.store.get(&self.tenant_id, logical, &next.key)?;
        if let Some(current) = current {
            match self.store.compare_swap(
                &self.tenant_id,
                logical,
                current.row_id,
                current.version,
                &next,
                None,
            )? {
                CompareSwap::Updated { .. } => Ok(()),
                CompareSwap::Conflict { .. } | CompareSwap::NotFound => Err(AelioError::new(
                    ReasonCode::Conflict,
                    "concurrent durable state update",
                )),
            }
        } else {
            match self
                .store
                .put_if_absent(&self.tenant_id, logical, &next, None)?
            {
                PutIfAbsent::Inserted { .. } => Ok(()),
                PutIfAbsent::Existing { .. } => Err(AelioError::new(
                    ReasonCode::Conflict,
                    "concurrent durable state creation",
                )),
            }
        }
    }
}

/// Validates a complete catalog snapshot before any durable or active state is changed.
fn promoted_version_is_better(
    candidate: &PromotedProcedureVersion,
    current: &PromotedProcedureVersion,
) -> bool {
    let candidate_rate =
        candidate.evidence.successes as f64 / candidate.evidence.observations.max(1) as f64;
    let current_rate =
        current.evidence.successes as f64 / current.evidence.observations.max(1) as f64;
    candidate_rate
        .total_cmp(&current_rate)
        .then(
            candidate
                .evidence
                .observations
                .cmp(&current.evidence.observations),
        )
        .then_with(|| {
            current
                .spec
                .evidence
                .mean_cost
                .total_cmp(&candidate.spec.evidence.mean_cost)
        })
        .then_with(|| candidate.promoted_at_ms.cmp(&current.promoted_at_ms))
        .then_with(|| candidate.version.cmp(&current.version))
        .is_gt()
}

fn procedure_artifact_binding_key(procedure_id: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(procedure_id.as_bytes());
    format!("adaptive-binding-{}", hex::encode(&digest[..16]))
}

fn flow_artifact_binding_key(flow_id: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(flow_id.as_bytes());
    format!("flow-binding-{}", hex::encode(&digest[..16]))
}

fn flow_trigger_overlap(left: &crate::tenant::FlowSpec, right: &crate::tenant::FlowSpec) -> bool {
    left.activation.trigger_surface.iter().any(|left_trigger| {
        let left_trigger = left_trigger.trim().to_ascii_lowercase();
        !left_trigger.is_empty()
            && right
                .activation
                .trigger_surface
                .iter()
                .any(|right_trigger| {
                    let right_trigger = right_trigger.trim().to_ascii_lowercase();
                    !right_trigger.is_empty()
                        && (left_trigger == right_trigger
                            || left_trigger.contains(&right_trigger)
                            || right_trigger.contains(&left_trigger))
                })
    })
}

pub fn validate_catalog(tenant: &TenantDecl) -> AelioResult<()> {
    if tenant.tenant_id.trim().is_empty()
        || tenant.tenant_id.len() > 256
        || tenant.tools.len() > 10_000
        || tenant.states.is_empty()
        || tenant.states.len() > 1_000
        || tenant.personalities.is_empty()
        || tenant.personalities.len() > 100
        || tenant.policies.len() > 10_000
        || tenant.flows.len() > 1_000
        || tenant.flow_artifacts.len() > tenant.flows.len()
        || tenant.harness_programs.len() > 1_000
        || tenant.attributes.len() > 10_000
    {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "catalog identity, required declarations, or collection bounds are invalid",
        ));
    }
    for (id, program) in &tenant.harness_programs {
        if id != &program.id {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("harness program map key `{id}` must match program.id `{}`", program.id),
            ));
        }
        program.validate()?;
    }
    let mut tool_ids = std::collections::HashSet::new();
    let mut capabilities = std::collections::HashMap::<&str, usize>::new();
    for tool in &tenant.tools {
        if tool.id.trim().is_empty()
            || tool.name.trim().is_empty()
            || tool.version.trim().is_empty()
            || tool.capability_tags.is_empty()
            || tool.id.len() > 256
            || tool.name.len() > 256
            || tool.version.len() > 128
            || tool.capability_tags.len() > 64
            || tool.params.len() > 256
            || tool.output_semantics.fields.len() > 256
            || tool.continuations.len() > 256
            || tool.errors.len() > 256
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "every tool requires id, name, version, and at least one capability",
            ));
        }
        if tool.effect.is_some()
            && tool.effectful
                != matches!(
                    tool.effect_class(),
                    crate::tenant::ToolEffect::Write | crate::tenant::ToolEffect::External
                )
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "tool {} effectful flag contradicts its exact effect class",
                    tool.id
                ),
            ));
        }
        if !tool_ids.insert(tool.id.as_str()) {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                format!("duplicate tool id {}", tool.id),
            ));
        }
        let mut tool_capabilities = std::collections::HashSet::new();
        for capability in &tool.capability_tags {
            if capability.trim().is_empty()
                || capability.len() > 256
                || !valid_declared_name(capability)
                || !tool_capabilities.insert(capability.as_str())
            {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!("tool {} has an empty or duplicate capability", tool.id),
                ));
            }
            *capabilities.entry(capability.as_str()).or_default() += 1;
        }
        let mut params = std::collections::HashSet::new();
        for param in &tool.params {
            if param.name.trim().is_empty()
                || param.type_name.trim().is_empty()
                || param.name.len() > 256
                || param.type_name.len() > 128
                || param.depends_on.len() > 64
                || !params.insert(param.name.as_str())
            {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!("tool {} has an invalid or duplicate parameter", tool.id),
                ));
            }
            if param.depends_on.iter().any(|name| name == &param.name) {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!(
                        "tool {} parameter {} depends on itself",
                        tool.id, param.name
                    ),
                ));
            }
            if let Some(crate::tenant::ParamConstraint::Pattern { regex }) = &param.constraint {
                regex::Regex::new(regex).map_err(|cause| {
                    AelioError::new(
                        ReasonCode::Validation,
                        format!(
                            "tool {} parameter {} has invalid regex: {cause}",
                            tool.id, param.name
                        ),
                    )
                })?;
            }
            if let crate::tenant::ParamSource::Derived { expr } = &param.source {
                let reference = expr
                    .strip_prefix("slot:")
                    .or_else(|| expr.strip_prefix("env:"));
                if !reference.is_some_and(valid_declared_name) {
                    return Err(AelioError::new(
                        ReasonCode::Validation,
                        format!(
                            "tool {} parameter {} has unsupported derived expression `{expr}`",
                            tool.id, param.name
                        ),
                    ));
                }
            }
        }
        for (name, field) in &tool.output_semantics.fields {
            if !valid_declared_name(name)
                || !valid_output_path(&field.path)
                || name.len() > 256
                || field.path.len() > 512
                || field.type_name.trim().is_empty()
                || field.type_name.len() > 128
                || field.meaning.trim().is_empty()
                || field.meaning.len() > 2_048
            {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!("tool {} has an invalid output declaration", tool.id),
                ));
            }
        }
        for param in &tool.params {
            if let Some(missing) = param
                .depends_on
                .iter()
                .find(|dependency| !params.contains(dependency.as_str()))
            {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!(
                        "tool {} parameter {} depends on unknown parameter {missing}",
                        tool.id, param.name
                    ),
                ));
            }
        }
    }
    if let Some((capability, _)) = capabilities.into_iter().find(|(_, count)| *count > 1) {
        return Err(AelioError::new(
            ReasonCode::Ambiguous,
            format!("capability {capability} resolves to multiple tools"),
        ));
    }
    let declared_capabilities: std::collections::HashSet<&str> = tenant
        .tools
        .iter()
        .flat_map(|tool| tool.capability_tags.iter().map(String::as_str))
        .collect();
    for tool in &tenant.tools {
        if let Some(continuation) = tool
            .continuations
            .iter()
            .find(|capability| !declared_capabilities.contains(capability.as_str()))
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "tool {} declares unknown continuation {continuation}",
                    tool.id
                ),
            ));
        }
    }
    let mut state_ids = std::collections::HashSet::new();
    for state in &tenant.states {
        if state.id.trim().is_empty() || !state_ids.insert(state.id.as_str()) {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                "state ids must be non-empty and unique",
            ));
        }
    }
    for state in &tenant.states {
        if let Some(direction) = &state.direction {
            if !state_ids.contains(direction.target.as_str()) {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!(
                        "state {} points at unknown direction target {}",
                        state.id, direction.target
                    ),
                ));
            }
        }
        for predicate in &state.entry_conditions {
            validate_closed_predicate(predicate)?;
        }
        for edge in &state.exit_edges {
            validate_closed_predicate(&edge.guard)?;
            if !state_ids.contains(edge.to.as_str()) {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!(
                        "state {} references unknown exit state {}",
                        state.id, edge.to
                    ),
                ));
            }
        }
        if let Some(timeout) = &state.timeout {
            if timeout.after_secs == 0 || !state_ids.contains(timeout.to.as_str()) {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!("state {} has an invalid timeout target", state.id),
                ));
            }
        }
    }
    let mut flow_ids = std::collections::HashSet::new();
    for flow in &tenant.flows {
        if flow.id.trim().is_empty()
            || flow.version.trim().is_empty()
            || !flow_ids.insert(flow.id.as_str())
        {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                format!("invalid or duplicate flow id {}", flow.id),
            ));
        }
        if flow.steps.is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("flow {} has no steps", flow.id),
            ));
        }
        if flow.steps.len() > 256
            || flow.activation.trigger_surface.is_empty()
            || flow.activation.trigger_surface.len() > 256
            || flow
                .activation
                .trigger_surface
                .iter()
                .any(|trigger| trigger.trim().is_empty() || trigger.len() > 512)
            || flow.max_attempts > 1_000
            || flow
                .ttl_secs
                .is_some_and(|ttl| ttl == 0 || ttl > 31_536_000)
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("flow {} exceeds structural execution bounds", flow.id),
            ));
        }
        let mut step_ids = std::collections::HashSet::new();
        if flow
            .steps
            .iter()
            .any(|step| step.id.trim().is_empty() || !step_ids.insert(step.id.as_str()))
        {
            return Err(AelioError::new(
                ReasonCode::Conflict,
                format!("flow {} has invalid or duplicate step ids", flow.id),
            ));
        }
        if flow.max_attempts == 0 {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("flow {} max_attempts must be positive", flow.id),
            ));
        }
        for predicate in &flow.activation.hard_preconditions {
            validate_closed_predicate(predicate)?;
        }
        for step in &flow.steps {
            validate_closed_predicate(&step.postcondition)?;
            let mut admissible = std::collections::HashSet::new();
            for capability in &step.admissible {
                if !declared_capabilities.contains(capability.as_str())
                    || !admissible.insert(capability)
                {
                    return Err(AelioError::new(
                        ReasonCode::Validation,
                        format!(
                            "flow {} step {} references an unknown or duplicate capability {capability}",
                            flow.id, step.id
                        ),
                    ));
                }
            }
        }
        if let Some(lowering) = &flow.lowering {
            validate_flow_lowering(flow, lowering)?;
        }
    }
    for (flow_id, artifact) in &tenant.flow_artifacts {
        if !flow_ids.contains(flow_id.as_str()) {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("runtime artifact binding references unknown flow {flow_id}"),
            ));
        }
        artifact.validate()?;
    }
    for flow in &tenant.flows {
        if let crate::tenant::FlowEscape::Fallback { flow_id } = &flow.escape {
            if !flow_ids.contains(flow_id.as_str()) {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!("flow {} references unknown fallback {}", flow.id, flow_id),
                ));
            }
        }
        if flow
            .terminal_states
            .iter()
            .any(|state| !state_ids.contains(state.as_str()))
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("flow {} references an unknown terminal state", flow.id),
            ));
        }
    }
    validate_unique_ids(
        tenant.personalities.iter().map(|item| item.id.as_str()),
        "personality",
    )?;
    validate_unique_ids(
        tenant.policies.iter().map(|item| item.id.as_str()),
        "policy",
    )?;
    for policy in &tenant.policies {
        if policy.id.len() > 256
            || policy.reason_code.trim().is_empty()
            || policy.reason_code.len() > 256
            || !(-1_000_000..=1_000_000).contains(&policy.priority)
            || policy
                .subject
                .state
                .as_ref()
                .is_some_and(|state| !state_ids.contains(state.as_str()))
            || policy
                .subject
                .tenant
                .as_ref()
                .is_some_and(|value| value != &tenant.tenant_id)
            || policy
                .action
                .tool_id
                .as_ref()
                .is_some_and(|tool| !tool_ids.contains(tool.as_str()))
            || policy.action.flow_id.as_ref().is_some_and(|flow| {
                !flow_ids.contains(flow.as_str())
                    && !matches!(
                        flow.as_str(),
                        "__term_confirmation__" | "__multi_clause_confirmation__"
                    )
            })
            || policy.action.capability.as_ref().is_some_and(|capability| {
                !declared_capabilities.contains(capability.as_str())
                    && !matches!(capability.as_str(), "memory.write" | "memory.delete")
            })
            || policy
                .subject
                .role
                .as_ref()
                .is_some_and(|value| !valid_declared_name(value))
            || policy
                .subject
                .segment
                .as_ref()
                .is_some_and(|value| !valid_declared_name(value))
            || policy
                .action
                .transition
                .as_ref()
                .is_some_and(|value| !valid_declared_name(value))
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "policy {} references an invalid or undeclared target",
                    policy.id
                ),
            ));
        }
        validate_closed_predicate(&policy.condition)?;
    }
    validate_unique_ids(
        tenant.attributes.iter().map(|item| item.name.as_str()),
        "attribute",
    )?;
    for attribute in &tenant.attributes {
        if attribute.anchors.is_empty()
            || attribute.anchors.len() > 256
            || attribute.learned.len() > 1_000
            || attribute
                .anchors
                .iter()
                .chain(&attribute.learned)
                .any(|term| term.trim().is_empty() || term.len() > 512)
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "attribute {} requires at least one semantic anchor",
                    attribute.name
                ),
            ));
        }
        if attribute.sortable && attribute.query_capability.is_none() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "sortable attribute {} requires a declared query capability",
                    attribute.name
                ),
            ));
        }
        if let Some(capability) = &attribute.query_capability {
            if !declared_capabilities.contains(capability.as_str()) {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!(
                        "attribute {} references unknown query capability {capability}",
                        attribute.name
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_flow_lowering(
    flow: &crate::tenant::FlowSpec,
    lowering: &crate::tenant::FlowLoweringV1,
) -> AelioResult<()> {
    const MAX_PROGRAM_BYTES: usize = 1024 * 1024;
    if lowering.format != 1
        || lowering.bindings.len() > 256
        || lowering.cases.len() < 20
        || lowering.cases.len() > 128
        || serde_json::to_vec(&lowering.program)
            .map(|encoded| encoded.len() > MAX_PROGRAM_BYTES)
            .unwrap_or(true)
    {
        return Err(AelioError::new(
            ReasonCode::Validation,
            format!("flow {} has an invalid or unbounded lowering", flow.id),
        ));
    }
    let steps: std::collections::HashMap<_, _> = flow
        .steps
        .iter()
        .map(|step| (step.id.as_str(), step))
        .collect();
    let mut bindings = std::collections::HashMap::new();
    for binding in &lowering.bindings {
        let Some(step) = steps.get(binding.step_id.as_str()) else {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "flow {} lowering binding {} references an unknown semantic step",
                    flow.id, binding.name
                ),
            ));
        };
        if !valid_declared_name(&binding.name)
            || binding.name.len() > 128
            || binding.deadline_ms == 0
            || binding.deadline_ms > 300_000
            || !step.admissible.contains(&binding.capability)
            || bindings.insert(binding.name.as_str(), binding).is_some()
        {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "flow {} lowering binding {} is invalid, duplicate, or not admissible",
                    flow.id, binding.name
                ),
            ));
        }
    }
    let mut calls = Vec::new();
    collect_lowering_call_ids(&lowering.program, &mut calls)?;
    let mut used = std::collections::HashSet::new();
    for call in calls {
        let Some(name) = call.strip_prefix("$cap:") else {
            return Err(AelioError::new(
                ReasonCode::Denied,
                format!(
                    "flow {} lowering contains a raw executable target id",
                    flow.id
                ),
            ));
        };
        if !bindings.contains_key(name) {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "flow {} lowering references unknown capability binding {name}",
                    flow.id
                ),
            ));
        }
        used.insert(name);
    }
    if used.len() != bindings.len() {
        return Err(AelioError::new(
            ReasonCode::Validation,
            format!("flow {} lowering has an unused capability binding", flow.id),
        ));
    }
    for case in &lowering.cases {
        if case.wakes.len() > 64 || case.fixtures.len() > 256 || case.expected.is_null() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!("flow {} lowering has an invalid gate case", flow.id),
            ));
        }
        for fixture in &case.fixtures {
            if !bindings.contains_key(fixture.binding.as_str()) {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    format!(
                        "flow {} gate fixture references unknown binding {}",
                        flow.id, fixture.binding
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn collect_lowering_call_ids<'a>(
    value: &'a serde_json::Value,
    calls: &mut Vec<&'a str>,
) -> AelioResult<()> {
    match value {
        serde_json::Value::Object(object) => {
            if object.get("op").and_then(serde_json::Value::as_str) == Some("Call") {
                let id = object
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::Validation,
                            "lowered Call requires a literal symbolic id",
                        )
                    })?;
                calls.push(id);
            }
            for child in object.values() {
                collect_lowering_call_ids(child, calls)?;
            }
        }
        serde_json::Value::Array(array) => {
            for child in array {
                collect_lowering_call_ids(child, calls)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn valid_declared_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.'))
}

fn valid_output_path(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '[' | ']' | '$')
        })
        && !value.contains("..")
}

fn validate_closed_predicate(predicate: &crate::contract::Predicate) -> AelioResult<()> {
    let mut nodes = 0_usize;
    validate_closed_predicate_inner(predicate, 0, &mut nodes)
}

fn validate_closed_predicate_inner(
    predicate: &crate::contract::Predicate,
    depth: usize,
    nodes: &mut usize,
) -> AelioResult<()> {
    use crate::contract::Predicate;

    *nodes = nodes.saturating_add(1);
    if depth > 32 || *nodes > 4_096 {
        return Err(AelioError::new(
            ReasonCode::Validation,
            "policy predicate exceeds depth or node bounds",
        ));
    }
    match predicate {
        Predicate::True | Predicate::False => Ok(()),
        Predicate::Eq { path, .. }
        | Predicate::Ne { path, .. }
        | Predicate::Gt { path, .. }
        | Predicate::Gte { path, .. }
        | Predicate::Lt { path, .. }
        | Predicate::Lte { path, .. }
        | Predicate::Present { path }
        | Predicate::Absent { path } => validate_predicate_path(path),
        Predicate::In { path, values } => {
            if values.len() > 256 {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    "policy membership predicate exceeds its value bound",
                ));
            }
            validate_predicate_path(path)
        }
        Predicate::And { of } | Predicate::Or { of } => {
            if of.is_empty() || of.len() > 64 {
                return Err(AelioError::new(
                    ReasonCode::Validation,
                    "and/or policy predicates must contain from 1 through 64 children",
                ));
            }
            for child in of {
                validate_closed_predicate_inner(child, depth + 1, nodes)?;
            }
            Ok(())
        }
        Predicate::Not { of } => validate_closed_predicate_inner(of, depth + 1, nodes),
    }
}

fn validate_predicate_path(path: &str) -> AelioResult<()> {
    let (root, child) = path
        .split_once('.')
        .map_or((path, None), |(root, child)| (root, Some(child)));
    let valid = match root {
        "state" | "role" | "tenant" | "segment" | "tool" | "capability" | "consent"
        | "transition" | "flow_id" => child.is_none(),
        "slot" | "env" | "budget" | "evidence" | "flow_context" => {
            child.is_some_and(|value| !value.is_empty() && !value.contains(".."))
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(AelioError::new(
            ReasonCode::Validation,
            format!("predicate path `{path}` is outside the closed policy grammar"),
        ))
    }
}

fn validate_unique_ids<'a>(
    values: impl IntoIterator<Item = &'a str>,
    kind: &str,
) -> AelioResult<()> {
    let mut seen = std::collections::HashSet::new();
    if values
        .into_iter()
        .any(|value| value.trim().is_empty() || !seen.insert(value))
    {
        return Err(AelioError::new(
            ReasonCode::Conflict,
            format!("{kind} ids must be non-empty and unique"),
        ));
    }
    Ok(())
}

fn envelope<T>(
    key: impl Into<String>,
    kind: &str,
    status: &str,
    owner: &str,
    now: i64,
    value: T,
) -> RecordEnvelope<T> {
    RecordEnvelope {
        key: key.into(),
        kind: kind.into(),
        status: status.into(),
        owner: owner.into(),
        created_at_ms: now,
        updated_at_ms: now,
        expires_at_ms: None,
        value,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use aelio_db_query::Database;

    use super::*;

    #[test]
    fn redaction_preserves_hashes_but_removes_bounded_phone_numbers() {
        let hash = "a3a8429724f595ca5ccc614ee46cf9e456d728a8b8f81183930110be9f72476b";
        assert_eq!(redact_text(hash), hash);
        assert_eq!(
            redact_text("phone=+919876543210; ok"),
            "phone=[REDACTED:pii]; ok"
        );
        assert_eq!(
            redact_text("call 555 123 4567 now"),
            "call [REDACTED:pii] now"
        );
    }

    fn runtime(tag: &str) -> DurableRuntime {
        let path = std::env::temp_dir().join(format!(
            "aelio_durable_runtime_{tag}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let db = Database::create(path).unwrap();
        let store = AelioStore::new(db, 3).unwrap();
        DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap()
    }

    struct CountingHost {
        calls: Arc<AtomicUsize>,
    }

    impl CapabilityHost for CountingHost {
        fn call_with_context(
            &mut self,
            tool: &ToolSpec,
            args: &IndexMap<String, Value>,
            _idempotency_key: &str,
            _user_id: &str,
            _channel: &str,
        ) -> AelioResult<Value> {
            let _ = (tool, args);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Map(indexmap::indexmap! {
                "ok".into() => Value::Bool(true),
            }))
        }
    }

    fn unique_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "aelio_durable_{tag}_{}_{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ))
    }

    fn directory_contains(path: &std::path::Path, needle: &[u8]) -> bool {
        let Ok(entries) = std::fs::read_dir(path) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                directory_contains(&path, needle)
            } else {
                std::fs::read(path)
                    .ok()
                    .is_some_and(|bytes| bytes.windows(needle.len()).any(|part| part == needle))
            }
        })
    }

    #[test]
    fn completed_turn_replays_without_reinvoking_tool() {
        let mut runtime = runtime("replay");
        let request = DurableTurnRequest {
            turn_id: "turn-1".into(),
            user_id: "u1".into(),
            utterance: "login with +919876543210".into(),
        };
        let first = runtime.run_turn(request.clone()).unwrap();
        let calls = runtime.world.tool_host.invocation_count();
        let replay = runtime.run_turn(request).unwrap();
        assert_eq!(runtime.world.tool_host.invocation_count(), calls);
        assert_eq!(replay.reply.text, first.reply.text);
    }

    #[test]
    fn external_state_commands_are_durable_idempotent_and_conflict_safe() {
        let mut runtime = runtime("external_state");
        assert!(!runtime
            .set_user_state("cmd-1", "user-1", "authenticated", Some("crm sync"))
            .unwrap());
        assert_eq!(
            runtime.world.user_state.get("user-1").map(String::as_str),
            Some("authenticated")
        );
        assert!(runtime
            .set_user_state("cmd-1", "user-1", "authenticated", Some("crm sync"))
            .unwrap());

        let conflict = runtime
            .set_user_state("cmd-1", "user-1", "unauthenticated", Some("different"))
            .unwrap_err();
        assert_eq!(conflict.code, ReasonCode::Conflict);

        runtime.world.user_state.clear();
        runtime.hydrate_user("user-1").unwrap();
        assert_eq!(
            runtime.world.user_state.get("user-1").map(String::as_str),
            Some("authenticated")
        );
    }

    #[test]
    fn imported_procedure_without_cold_proposal_does_not_fail_the_turn() {
        let path = unique_path("imported_procedure");
        std::fs::create_dir_all(&path).unwrap();
        let store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
        let mut world = World::demo_tenant("tenant-1");
        let state = world
            .tenant
            .states
            .iter()
            .find(|state| state.id == "unauthenticated")
            .unwrap();
        let sigma = crate::abilities::learn::situation_key(
            "unauthenticated",
            "greeting",
            vec![],
            state.permission_envelope.clone(),
            None,
            0,
            None,
        );
        world
            .registry
            .register_procedure(crate::abilities::registry::ProcedureSpec {
                id: "imported-greeting".into(),
                version: "1".into(),
                tenant_id: "tenant-1".into(),
                situation_hash: crate::abilities::learn::situation_hash(&sigma),
                situation_filter: crate::abilities::registry::SituationFilter {
                    state: Some("unauthenticated".into()),
                    intent_class: Some("greeting".into()),
                    ..Default::default()
                },
                situation_embedding: vec![],
                path: crate::contract::AbilityPath::seq([
                    "State.Direction",
                    "Registry.Capabilities",
                    "Express.Template",
                ]),
                contract: AbilityContract::pure("imported-greeting"),
                tool_deps: vec![],
                prompt_deps: vec![],
                evidence: crate::abilities::registry::ProcedureEvidence {
                    observations: 10,
                    success_rate: 1.0,
                    ..Default::default()
                },
                status: crate::abilities::registry::ProcedureStatus::Promoted,
                provenance: crate::abilities::registry::ProcedureProvenance {
                    origin: "catalog_import".into(),
                    proposed_by: "operator".into(),
                    approved_by: Some("operator".into()),
                },
                supersedes: None,
            });
        let mut runtime = DurableRuntime::new(world, store).unwrap();
        let result = runtime
            .run_turn(DurableTurnRequest {
                turn_id: "turn-imported".into(),
                user_id: "u1".into(),
                utterance: "hi".into(),
            })
            .unwrap();
        assert_eq!(result.tier, Some(crate::LookupTier::Tier0));
        assert!(runtime
            .store
            .get::<ColdProposal>("tenant-1", LogicalTable::Proposals, "imported-greeting")
            .unwrap()
            .is_none());
    }

    #[test]
    fn turn_idempotency_key_cannot_cross_users_or_payloads() {
        let mut runtime = runtime("replay_binding");
        runtime
            .run_turn(DurableTurnRequest {
                turn_id: "shared-turn-id".into(),
                user_id: "user-a".into(),
                utterance: "hi".into(),
            })
            .unwrap();
        for request in [
            DurableTurnRequest {
                turn_id: "shared-turn-id".into(),
                user_id: "user-b".into(),
                utterance: "hi".into(),
            },
            DurableTurnRequest {
                turn_id: "shared-turn-id".into(),
                user_id: "user-a".into(),
                utterance: "show private account details".into(),
            },
        ] {
            assert_eq!(
                runtime.run_turn(request).unwrap_err().code,
                ReasonCode::Conflict
            );
        }
    }

    #[test]
    fn turn_request_fields_are_bounded_before_storage() {
        let mut runtime = runtime("turn_bounds");
        let error = runtime
            .run_turn(DurableTurnRequest {
                turn_id: "turn-too-large".into(),
                user_id: "user-1".into(),
                utterance: "x".repeat(32_769),
            })
            .unwrap_err();
        assert_eq!(error.code, ReasonCode::Validation);
        assert!(runtime.get_turn("turn-too-large").unwrap().is_none());
    }

    #[test]
    fn failed_turn_replays_the_durable_error_instead_of_staying_processing() {
        let mut runtime = runtime("failed_turn_replay");
        let now = Utc::now().timestamp_millis();
        runtime
            .store
            .put_if_absent(
                "tenant-1",
                LogicalTable::Turns,
                &envelope(
                    "turn-failed",
                    "turn",
                    "failed",
                    "runtime",
                    now,
                    DurableTurnValue {
                        user_id: "user-1".into(),
                        request_hash: String::new(),
                        result: None,
                        error: Some(AelioError::new(
                            ReasonCode::Unavailable,
                            "storage unavailable",
                        )),
                    },
                ),
                None,
            )
            .unwrap();
        let error = runtime
            .run_turn(DurableTurnRequest {
                turn_id: "turn-failed".into(),
                user_id: "user-1".into(),
                utterance: "hi".into(),
            })
            .unwrap_err();
        assert_eq!(error.code, ReasonCode::Unavailable);
    }

    #[test]
    fn active_catalog_is_reloaded_after_runtime_restart() {
        let mut runtime = runtime("catalog_restart");
        let shared = runtime.store.clone();
        let mut catalog = runtime.world.tenant.clone();
        catalog.attributes[0].learned.push("penny-pinching".into());
        runtime.register_catalog(catalog).unwrap();
        drop(runtime);

        let restarted = DurableRuntime::new(World::demo_tenant("tenant-1"), shared).unwrap();
        assert!(restarted.world.tenant.attributes[0]
            .learned
            .iter()
            .any(|term| term == "penny-pinching"));
    }

    #[test]
    fn otp_flow_state_is_durable_between_turns() {
        let mut runtime = runtime("otp");
        let first = runtime
            .run_turn(DurableTurnRequest {
                turn_id: "turn-1".into(),
                user_id: "u1".into(),
                utterance: "login with +919876543210".into(),
            })
            .unwrap();
        assert!(first.suspended);
        let second = runtime
            .run_turn(DurableTurnRequest {
                turn_id: "turn-2".into(),
                user_id: "u1".into(),
                utterance: "434543".into(),
            })
            .unwrap();
        assert_eq!(second.new_state.as_deref(), Some("authenticated"));
        assert_eq!(runtime.world.tool_host.invocation_count(), Some(2));
    }

    #[test]
    fn turn_steps_and_provider_calls_are_persisted() {
        let mut runtime = runtime("steps");
        let result = runtime
            .run_turn(DurableTurnRequest {
                turn_id: "turn-steps".into(),
                user_id: "u1".into(),
                utterance: "Hi".into(),
            })
            .unwrap();
        assert!(!result.steps.is_empty());
        let stored = runtime.list_steps("turn-steps").unwrap();
        assert_eq!(stored.len(), result.steps.len());
        assert_eq!(stored[0].envelope.value.name, result.steps[0].name);
        // Scripted provider records ProposePath call(s)
        let calls = runtime.list_calls("turn-steps").unwrap();
        assert_eq!(calls.len(), result.llm_calls as usize);
        let reloaded = runtime.get_turn("turn-steps").unwrap().unwrap();
        assert_eq!(reloaded.reply.text, result.reply.text);
    }

    #[test]
    fn durable_payloads_omit_phone_and_otp_bytes() {
        let path = unique_path("redaction");
        std::fs::create_dir_all(&path).unwrap();
        let store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
        let mut runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
        let phone = "+919876543210";
        let otp = "434543";
        runtime
            .run_turn(DurableTurnRequest {
                turn_id: "redact-1".into(),
                user_id: "u-redact".into(),
                utterance: format!("login with {phone}"),
            })
            .unwrap();
        runtime
            .run_turn(DurableTurnRequest {
                turn_id: "redact-2".into(),
                user_id: "u-redact".into(),
                utterance: otp.into(),
            })
            .unwrap();
        runtime.store.flush().unwrap();
        drop(runtime);
        assert!(!directory_contains(&path, phone.as_bytes()));
        assert!(!directory_contains(&path, otp.as_bytes()));
    }

    #[test]
    fn durable_idempotency_replays_result_after_restart() {
        let path = unique_path("idempotency_restart");
        std::fs::create_dir_all(&path).unwrap();
        let mut store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
        store.migrate_tenant("tenant-1", 0).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool = World::demo_tenant("tenant-1").tenant.tools[2].clone();
        let args = indexmap::indexmap! {
            "limit".into() => Value::Int(10),
        };
        {
            let mut host = DurableToolHost {
                tenant_id: "tenant-1".into(),
                owner: "first".into(),
                store: store.clone(),
                inner: Box::new(CountingHost {
                    calls: Arc::clone(&calls),
                }),
            };
            host.call_with_context(&tool, &args, "idem-restart", "user-1", "test")
                .unwrap();
        }
        store.flush().unwrap();
        drop(store);

        let reopened = AelioStore::new(Database::open(&path).unwrap(), 3).unwrap();
        let mut host = DurableToolHost {
            tenant_id: "tenant-1".into(),
            owner: "second".into(),
            store: reopened.clone(),
            inner: Box::new(CountingHost {
                calls: Arc::clone(&calls),
            }),
        };
        host.call_with_context(&tool, &args, "idem-restart", "user-1", "test")
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let row: StoredRecord<DurableIdempotencyRecord> = reopened
            .get("tenant-1", LogicalTable::Idempotency, "idem-restart")
            .unwrap()
            .unwrap();
        assert_eq!(row.envelope.value.state, DurableIdempotencyState::Completed);
    }

    #[test]
    fn sdk_delivery_dispositions_are_redacted_and_survive_restart() {
        let path = unique_path("sdk_delivery_audit_restart");
        std::fs::create_dir_all(&path).unwrap();
        let raw_correlation = "sdk-correlation-that-must-not-be-persisted";
        {
            let store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
            let mut runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
            runtime
                .record_sdk_delivery_disposition(raw_correlation, "accepted")
                .unwrap();
            runtime
                .record_sdk_delivery_disposition(raw_correlation, "duplicate")
                .unwrap();
            runtime.store.flush().unwrap();
        }

        let store = AelioStore::new(Database::open(&path).unwrap(), 3).unwrap();
        let runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
        let rows = runtime.list_sdk_delivery_dispositions(10).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|row| row.envelope.value.correlation_hash.len() == 64));
        assert!(rows.iter().all(|row| {
            !serde_json::to_string(&row.envelope)
                .unwrap()
                .contains(raw_correlation)
        }));
        assert_eq!(
            rows.iter()
                .map(|row| row.envelope.value.disposition.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["accepted", "duplicate"])
        );
    }

    #[test]
    fn authored_flow_artifact_binding_survives_runtime_restart() {
        let path = unique_path("flow_binding_restart");
        std::fs::create_dir_all(&path).unwrap();
        let pin = crate::adaptive::ArtifactPinV1 {
            id: "login.runtime".into(),
            version: 1,
            hash: "a".repeat(64),
        };
        {
            let store = AelioStore::new(Database::create(&path).unwrap(), 3).unwrap();
            let mut runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
            runtime
                .bind_flow_artifact("login", pin.clone(), "admin:test")
                .unwrap();
            runtime.store.flush().unwrap();
        }
        let store = AelioStore::new(Database::open(&path).unwrap(), 3).unwrap();
        let runtime = DurableRuntime::new(World::demo_tenant("tenant-1"), store).unwrap();
        assert_eq!(runtime.world.registry.flow_artifact("login"), Some(&pin));
    }

    #[test]
    fn unknown_non_idempotent_outcome_requires_manual_review() {
        let mut runtime = runtime("unknown_non_idempotent");
        let tool = runtime.world.tenant.tools[0].clone();
        let now = Utc::now().timestamp_millis();
        runtime
            .store
            .put_if_absent(
                "tenant-1",
                LogicalTable::Idempotency,
                &RecordEnvelope {
                    key: "unknown-effect".into(),
                    kind: "tool_idempotency".into(),
                    status: "processing".into(),
                    owner: "crashed".into(),
                    created_at_ms: now - 60_000,
                    updated_at_ms: now - 60_000,
                    expires_at_ms: Some(now - 1),
                    value: DurableIdempotencyRecord {
                        key: "unknown-effect".into(),
                        tool_id: tool.id.clone(),
                        tool_version: tool.version.clone(),
                        user_id: "user-1".into(),
                        idempotent: false,
                        state: DurableIdempotencyState::Processing,
                        attempts: 1,
                        lease_owner: "crashed".into(),
                        lease_expires_at_ms: Some(now - 1),
                        safe_result: None,
                        reason_code: None,
                        failure_message: None,
                    },
                },
                None,
            )
            .unwrap();
        let error = runtime
            .world
            .tool_host
            .call_with_context(&tool, &IndexMap::new(), "unknown-effect", "user-1", "test")
            .unwrap_err();
        assert_eq!(error.code, ReasonCode::NeedsEscalation);
        let row: StoredRecord<DurableIdempotencyRecord> = runtime
            .store
            .get("tenant-1", LogicalTable::Idempotency, "unknown-effect")
            .unwrap()
            .unwrap();
        assert_eq!(
            row.envelope.value.state,
            DurableIdempotencyState::ManualReview
        );
    }
}
