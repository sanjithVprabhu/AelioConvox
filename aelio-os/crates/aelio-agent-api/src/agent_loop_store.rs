//! Fenced, encrypted conversation state for the agent-loop harness.

use aelio_agent_loop::{AgentMessage, EffectClass, LoopManifest, OrchestrationState, ToolCall};
use aelio_sol::SolValue;
use aelio_store::{PutIfAbsent, Store, StoreError};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

const TABLE: &str = "agent_loop_conversations";
const RATE_LIMIT_TABLE: &str = "agent_loop_rate_limits";
const CANCELLATION_TABLE: &str = "agent_loop_cancellations";
const INVOCATION_TABLE: &str = "agent_loop_invocations";
const SCHEMA_VERSION: u32 = 1;
const MAX_SEALED_STATE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingConfirmation {
    pub confirmation_id: String,
    pub action_digest: String,
    pub origin_turn_id: String,
    pub calls: Vec<PendingCall>,
    pub requested_at_ms: i64,
    pub expires_at_ms: i64,
    #[serde(default)]
    pub batch_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingCall {
    pub call: ToolCall,
    pub tool_version: String,
    pub effect_class: EffectClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingInput {
    pub calls: Vec<ToolCall>,
    pub missing_fields: Vec<String>,
    pub requested_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ConversationState {
    pub system: String,
    pub bootstrap: String,
    pub manifest: LoopManifest,
    pub messages: Vec<AgentMessage>,
    pub pending_confirmation: Option<PendingConfirmation>,
    #[serde(default)]
    pub pending_input: Option<PendingInput>,
    #[serde(default)]
    pub continuation_capabilities: Vec<String>,
    #[serde(default)]
    pub continuation_state_id: Option<String>,
    pub last_turn_id_hash: Option<String>,
    #[serde(default)]
    pub last_request_hash: Option<String>,
    #[serde(default)]
    pub compaction_count: u32,
    pub last_result: Option<aelio_agent::blocks::turn::TurnResult>,
    /// Plan/execute board: todos, sub-harness tasks, stored programs, reflections.
    #[serde(default)]
    pub orchestration: Option<OrchestrationState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversationEnvelope {
    schema_version: u32,
    fence: u64,
    lease_holder: Option<String>,
    lease_expires_at_ms: Option<i64>,
    manifest_hash: Option<String>,
    sealed_state: Option<String>,
}

#[derive(Debug)]
pub(crate) struct ConversationLease {
    tenant_id: String,
    key: String,
    holder: String,
    pub fence: u64,
    version: u64,
    envelope: ConversationEnvelope,
    pub state: Option<ConversationState>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ConversationStoreError {
    #[error("conversation is already being processed")]
    Busy,
    #[error("conversation lease was lost")]
    LeaseLost,
    #[error("agent-loop state encryption key is invalid: {0}")]
    Key(String),
    #[error("agent-loop state is corrupt: {0}")]
    Corrupt(String),
    #[error("agent-loop store failed: {0}")]
    Store(String),
    #[error("agent-loop rate limit exceeded for {0}")]
    RateLimited(String),
    #[error("agent-loop invocation replay changed at {0}")]
    InvocationConflict(String),
}

#[derive(Debug, Clone, Copy)]
struct RateLimits {
    tenant_per_minute: u64,
    user_per_minute: u64,
    conversation_per_minute: u64,
    tool_per_minute: u64,
}

impl Default for RateLimits {
    fn default() -> Self {
        Self {
            tenant_per_minute: 10_000,
            user_per_minute: 120,
            conversation_per_minute: 100,
            tool_per_minute: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateCounter {
    window: i64,
    count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancellationRecord {
    fence: u64,
    requested_at_ms: i64,
    expires_at_ms: i64,
    active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvocationReservation {
    invocation_key: String,
    turn_id_hash: String,
    ordinal: usize,
    batch_sequence: u64,
    call_digest: String,
}

#[derive(Clone)]
pub(crate) struct StateKey([u8; 32]);

impl StateKey {
    pub fn from_environment() -> Result<Self, ConversationStoreError> {
        let raw = std::env::var("AELIO_AGENT_LOOP_STATE_KEY").map_err(|_| {
            ConversationStoreError::Key(
                "AELIO_AGENT_LOOP_STATE_KEY must be a 32-byte base64url or 64-character hex key"
                    .to_string(),
            )
        })?;
        Self::parse(&raw)
    }

    fn parse(raw: &str) -> Result<Self, ConversationStoreError> {
        let decoded = URL_SAFE_NO_PAD
            .decode(raw)
            .ok()
            .filter(|decoded| decoded.len() == 32)
            .or_else(|| hex::decode(raw).ok().filter(|decoded| decoded.len() == 32))
            .ok_or_else(|| {
                ConversationStoreError::Key("key encoding is not base64url or hex".to_string())
            })?;
        let bytes: [u8; 32] = decoded.try_into().map_err(|decoded: Vec<u8>| {
            ConversationStoreError::Key(format!("expected 32 bytes, got {}", decoded.len()))
        })?;
        Ok(Self(bytes))
    }

    #[cfg(test)]
    fn test_key() -> Self {
        Self([0x5a; 32])
    }

    fn seal<T: Serialize>(&self, value: &T, aad: &[u8]) -> Result<String, ConversationStoreError> {
        let mut plaintext = serde_json::to_vec(value)
            .map_err(|error| ConversationStoreError::Corrupt(error.to_string()))?;
        if plaintext.len() > MAX_SEALED_STATE_BYTES {
            return Err(ConversationStoreError::Corrupt(
                "conversation state exceeds the storage limit".to_string(),
            ));
        }
        let mut nonce_bytes = [0_u8; 12];
        SystemRandom::new().fill(&mut nonce_bytes).map_err(|_| {
            ConversationStoreError::Key("secure randomness unavailable".to_string())
        })?;
        key(&self.0)?
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad),
                &mut plaintext,
            )
            .map_err(|_| ConversationStoreError::Corrupt("state encryption failed".to_string()))?;
        Ok(format!(
            "v1.{}.{}",
            URL_SAFE_NO_PAD.encode(nonce_bytes),
            URL_SAFE_NO_PAD.encode(plaintext)
        ))
    }

    fn open<T: for<'de> Deserialize<'de>>(
        &self,
        sealed: &str,
        aad: &[u8],
    ) -> Result<T, ConversationStoreError> {
        let mut parts = sealed.split('.');
        if parts.next() != Some("v1") {
            return Err(ConversationStoreError::Corrupt(
                "unsupported sealed-state version".to_string(),
            ));
        }
        let nonce = parts
            .next()
            .ok_or_else(|| ConversationStoreError::Corrupt("missing nonce".to_string()))?;
        let ciphertext = parts
            .next()
            .ok_or_else(|| ConversationStoreError::Corrupt("missing ciphertext".to_string()))?;
        if parts.next().is_some() {
            return Err(ConversationStoreError::Corrupt(
                "invalid sealed-state envelope".to_string(),
            ));
        }
        let nonce: [u8; 12] = URL_SAFE_NO_PAD
            .decode(nonce)
            .map_err(|_| ConversationStoreError::Corrupt("invalid nonce".to_string()))?
            .try_into()
            .map_err(|_| ConversationStoreError::Corrupt("invalid nonce length".to_string()))?;
        let mut ciphertext = URL_SAFE_NO_PAD
            .decode(ciphertext)
            .map_err(|_| ConversationStoreError::Corrupt("invalid ciphertext".to_string()))?;
        if ciphertext.len() > MAX_SEALED_STATE_BYTES + AES_256_GCM.tag_len() {
            return Err(ConversationStoreError::Corrupt(
                "sealed state exceeds the storage limit".to_string(),
            ));
        }
        let plaintext = key(&self.0)?
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                &mut ciphertext,
            )
            .map_err(|_| {
                ConversationStoreError::Corrupt("state authentication failed".to_string())
            })?;
        serde_json::from_slice(plaintext)
            .map_err(|error| ConversationStoreError::Corrupt(error.to_string()))
    }
}

fn key(bytes: &[u8; 32]) -> Result<LessSafeKey, ConversationStoreError> {
    UnboundKey::new(&AES_256_GCM, bytes)
        .map(LessSafeKey::new)
        .map_err(|_| ConversationStoreError::Key("AES-256-GCM initialization failed".to_string()))
}

pub(crate) struct ConversationRepository<'a> {
    store: &'a mut dyn Store,
    key: StateKey,
}

impl<'a> ConversationRepository<'a> {
    pub fn new(store: &'a mut dyn Store, key: StateKey) -> Self {
        Self { store, key }
    }

    pub fn acquire(
        &mut self,
        tenant_id: &str,
        conversation_id: &str,
        holder_id: &str,
        now_ms: i64,
        ttl_ms: i64,
    ) -> Result<ConversationLease, ConversationStoreError> {
        if ttl_ms <= 0 {
            return Err(ConversationStoreError::Store(
                "conversation lease TTL must be positive".to_string(),
            ));
        }
        let conversation_key = digest(conversation_id);
        let holder = digest(holder_id);
        let envelope = ConversationEnvelope {
            schema_version: SCHEMA_VERSION,
            fence: 1,
            lease_holder: Some(holder.clone()),
            lease_expires_at_ms: Some(now_ms.saturating_add(ttl_ms)),
            manifest_hash: None,
            sealed_state: None,
        };
        match self
            .store
            .put_if_absent(tenant_id, TABLE, &conversation_key, encode(&envelope)?)
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { version } => Ok(ConversationLease {
                tenant_id: tenant_id.to_string(),
                key: conversation_key,
                holder,
                fence: 1,
                version,
                envelope,
                state: None,
            }),
            PutIfAbsent::Existing(existing) => {
                let current: ConversationEnvelope = decode(&existing.value)?;
                if current.schema_version != SCHEMA_VERSION {
                    return Err(ConversationStoreError::Corrupt(
                        "unsupported conversation schema".to_string(),
                    ));
                }
                if current.lease_holder.is_some()
                    && current
                        .lease_expires_at_ms
                        .is_some_and(|expiry| expiry > now_ms)
                {
                    return Err(ConversationStoreError::Busy);
                }
                let current_aad = state_aad(tenant_id, &conversation_key, current.fence);
                let state: Option<ConversationState> = current
                    .sealed_state
                    .as_deref()
                    .map(|sealed| self.key.open(sealed, &current_aad))
                    .transpose()?;
                let mut next = current.clone();
                next.fence = next.fence.saturating_add(1);
                next.lease_holder = Some(holder.clone());
                next.lease_expires_at_ms = Some(now_ms.saturating_add(ttl_ms));
                if let Some(state) = &state {
                    let next_aad = state_aad(tenant_id, &conversation_key, next.fence);
                    next.sealed_state = Some(self.key.seal(state, &next_aad)?);
                }
                let version = self
                    .store
                    .cas(
                        tenant_id,
                        TABLE,
                        &conversation_key,
                        existing.version,
                        encode(&next)?,
                    )
                    .map_err(|error| match error {
                        StoreError::Conflict => ConversationStoreError::Busy,
                        other => store_error(other),
                    })?;
                Ok(ConversationLease {
                    tenant_id: tenant_id.to_string(),
                    key: conversation_key,
                    holder,
                    fence: next.fence,
                    version,
                    envelope: next,
                    state,
                })
            }
        }
    }

    pub fn commit(
        &mut self,
        lease: ConversationLease,
        state: &ConversationState,
    ) -> Result<(), ConversationStoreError> {
        let aad = state_aad(&lease.tenant_id, &lease.key, lease.fence);
        let sealed_state = self.key.seal(state, &aad)?;
        let mut next = lease.envelope;
        next.lease_holder = None;
        next.lease_expires_at_ms = None;
        next.manifest_hash = Some(state.manifest.hash.clone());
        next.sealed_state = Some(sealed_state);
        self.store
            .cas(
                &lease.tenant_id,
                TABLE,
                &lease.key,
                lease.version,
                encode(&next)?,
            )
            .map_err(|error| match error {
                StoreError::Conflict => ConversationStoreError::LeaseLost,
                other => store_error(other),
            })?;
        Ok(())
    }

    pub fn release(&mut self, lease: ConversationLease) -> Result<(), ConversationStoreError> {
        let mut next = lease.envelope;
        if next.lease_holder.as_deref() != Some(lease.holder.as_str()) {
            return Err(ConversationStoreError::LeaseLost);
        }
        next.lease_holder = None;
        next.lease_expires_at_ms = None;
        self.store
            .cas(
                &lease.tenant_id,
                TABLE,
                &lease.key,
                lease.version,
                encode(&next)?,
            )
            .map_err(|error| match error {
                StoreError::Conflict => ConversationStoreError::LeaseLost,
                other => store_error(other),
            })?;
        Ok(())
    }
}

pub(crate) fn turn_hash(turn_id: &str) -> String {
    digest(turn_id)
}

/// Mark a cancellation only when this conversation currently has an active fenced turn. The
/// marker binds that exact fence so a late request can never cancel a later conversation turn.
pub(crate) fn request_active_cancellation(
    store: &mut dyn Store,
    tenant_id: &str,
    conversation_id: &str,
    now_ms: i64,
) -> Result<Option<u64>, ConversationStoreError> {
    let conversation_key = digest(conversation_id);
    let Some(row) = store
        .get(tenant_id, TABLE, &conversation_key)
        .map_err(store_error)?
    else {
        return Ok(None);
    };
    let envelope: ConversationEnvelope = decode(&row.value)?;
    if envelope.lease_holder.is_none()
        || envelope
            .lease_expires_at_ms
            .is_none_or(|expiry| expiry <= now_ms)
    {
        return Ok(None);
    }
    let record = CancellationRecord {
        fence: envelope.fence,
        requested_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(660_000),
        active: true,
    };
    upsert_cancellation(store, tenant_id, &conversation_key, &record)?;
    Ok(Some(envelope.fence))
}

pub(crate) fn cancellation_requested(
    store: &dyn Store,
    tenant_id: &str,
    conversation_id: &str,
    fence: u64,
    now_ms: i64,
) -> Result<bool, ConversationStoreError> {
    let key = digest(conversation_id);
    let Some(row) = store
        .get(tenant_id, CANCELLATION_TABLE, &key)
        .map_err(store_error)?
    else {
        return Ok(false);
    };
    let record: CancellationRecord = decode(&row.value)?;
    Ok(record.active && record.fence == fence && record.expires_at_ms > now_ms)
}

pub(crate) fn clear_cancellation(
    store: &mut dyn Store,
    tenant_id: &str,
    conversation_id: &str,
    fence: u64,
    now_ms: i64,
) -> Result<(), ConversationStoreError> {
    let key = digest(conversation_id);
    let Some(existing) = store
        .get(tenant_id, CANCELLATION_TABLE, &key)
        .map_err(store_error)?
    else {
        return Ok(());
    };
    let current: CancellationRecord = decode(&existing.value)?;
    if current.fence != fence {
        return Ok(());
    }
    let cleared = CancellationRecord {
        active: false,
        expires_at_ms: now_ms,
        ..current
    };
    store
        .cas(
            tenant_id,
            CANCELLATION_TABLE,
            &key,
            existing.version,
            encode(&cleared)?,
        )
        .map_err(store_error)?;
    Ok(())
}

fn upsert_cancellation(
    store: &mut dyn Store,
    tenant_id: &str,
    key: &str,
    record: &CancellationRecord,
) -> Result<(), ConversationStoreError> {
    for _ in 0..16 {
        match store
            .put_if_absent(tenant_id, CANCELLATION_TABLE, key, encode(record)?)
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => return Ok(()),
            PutIfAbsent::Existing(existing) => match store.cas(
                tenant_id,
                CANCELLATION_TABLE,
                key,
                existing.version,
                encode(record)?,
            ) {
                Ok(_) => return Ok(()),
                Err(StoreError::Conflict) => continue,
                Err(error) => return Err(store_error(error)),
            },
        }
    }
    Err(ConversationStoreError::Store(
        "cancellation marker remained contended".to_string(),
    ))
}

/// Reserve all four quota scopes for a model response before the SDK sees any call. Reservations
/// are intentionally conservative: a later scope rejection may leave earlier counters charged,
/// but it can never permit a partial external dispatch.
pub(crate) fn preflight_tool_batch(
    store: &mut dyn Store,
    tenant_id: &str,
    user_id: &str,
    conversation_id: &str,
    calls: &[ToolCall],
    now_ms: i64,
) -> Result<(), ConversationStoreError> {
    preflight_tool_batch_with_limits(
        store,
        tenant_id,
        user_id,
        conversation_id,
        calls,
        now_ms,
        RateLimits::default(),
    )
}

/// Persist the model's intended call sequence before any external dispatch. Provider call IDs are
/// deliberately excluded: a provider may assign fresh IDs after restart. Turn + ordinal owns the
/// effect identity, while `call_digest` prevents a nondeterministic replay from changing it.
pub(crate) fn reserve_invocation_batch(
    store: &mut dyn Store,
    tenant_id: &str,
    conversation_id: &str,
    turn_id: &str,
    batch_sequence: u64,
    calls: &[(ToolCall, String)],
) -> Result<Vec<(String, String)>, ConversationStoreError> {
    let turn_id_hash = digest(turn_id);
    let conversation_hash = digest(conversation_id);
    let mut keys = Vec::with_capacity(calls.len());
    for (ordinal, (call, version)) in calls.iter().enumerate() {
        let call_bytes = serde_json::to_vec(&(&call.name, version, &call.arguments))
            .map_err(|error| ConversationStoreError::Corrupt(error.to_string()))?;
        let call_digest = blake3::hash(&call_bytes).to_hex().to_string();
        let invocation_key = digest(&format!(
            "{conversation_hash}\u{1f}{turn_id_hash}\u{1f}{batch_sequence}\u{1f}{ordinal}"
        ));
        let reservation = InvocationReservation {
            invocation_key: invocation_key.clone(),
            turn_id_hash: turn_id_hash.clone(),
            ordinal,
            batch_sequence,
            call_digest,
        };
        match store
            .put_if_absent(
                tenant_id,
                INVOCATION_TABLE,
                &invocation_key,
                encode(&reservation)?,
            )
            .map_err(store_error)?
        {
            PutIfAbsent::Inserted { .. } => {}
            PutIfAbsent::Existing(existing) => {
                let prior: InvocationReservation = decode(&existing.value)?;
                if prior.turn_id_hash != reservation.turn_id_hash
                    || prior.ordinal != reservation.ordinal
                    || prior.batch_sequence != reservation.batch_sequence
                    || prior.call_digest != reservation.call_digest
                {
                    return Err(ConversationStoreError::InvocationConflict(format!(
                        "ordinal {ordinal}"
                    )));
                }
            }
        }
        keys.push((call.id.clone(), invocation_key));
    }
    Ok(keys)
}

fn preflight_tool_batch_with_limits(
    store: &mut dyn Store,
    tenant_id: &str,
    user_id: &str,
    conversation_id: &str,
    calls: &[ToolCall],
    now_ms: i64,
    limits: RateLimits,
) -> Result<(), ConversationStoreError> {
    for call in calls {
        consume_rate_scope(
            store,
            tenant_id,
            "tenant",
            "all",
            limits.tenant_per_minute,
            now_ms,
        )?;
        consume_rate_scope(
            store,
            tenant_id,
            "user",
            user_id,
            limits.user_per_minute,
            now_ms,
        )?;
        consume_rate_scope(
            store,
            tenant_id,
            "conversation",
            conversation_id,
            limits.conversation_per_minute,
            now_ms,
        )?;
        consume_rate_scope(
            store,
            tenant_id,
            "tool",
            &call.name,
            limits.tool_per_minute,
            now_ms,
        )?;
    }
    Ok(())
}

fn consume_rate_scope(
    store: &mut dyn Store,
    tenant_id: &str,
    scope: &str,
    identity: &str,
    limit: u64,
    now_ms: i64,
) -> Result<(), ConversationStoreError> {
    if limit == 0 {
        return Err(ConversationStoreError::RateLimited(scope.to_string()));
    }
    let key = digest(&format!("{scope}\u{1f}{identity}"));
    let window = now_ms.div_euclid(60_000);
    for _ in 0..16 {
        let Some(existing) = store
            .get(tenant_id, RATE_LIMIT_TABLE, &key)
            .map_err(store_error)?
        else {
            let counter = RateCounter { window, count: 1 };
            match store
                .put_if_absent(tenant_id, RATE_LIMIT_TABLE, &key, encode(&counter)?)
                .map_err(store_error)?
            {
                PutIfAbsent::Inserted { .. } => return Ok(()),
                PutIfAbsent::Existing(_) => continue,
            }
        };
        let current: RateCounter = decode(&existing.value)?;
        if current.window == window && current.count >= limit {
            return Err(ConversationStoreError::RateLimited(scope.to_string()));
        }
        let next = RateCounter {
            window,
            count: if current.window == window {
                current.count.saturating_add(1)
            } else {
                1
            },
        };
        match store.cas(
            tenant_id,
            RATE_LIMIT_TABLE,
            &key,
            existing.version,
            encode(&next)?,
        ) {
            Ok(_) => return Ok(()),
            Err(StoreError::Conflict) => continue,
            Err(error) => return Err(store_error(error)),
        }
    }
    Err(ConversationStoreError::Store(
        "rate-limit counter remained contended".to_string(),
    ))
}

fn digest(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}

fn state_aad(tenant: &str, key: &str, fence: u64) -> Vec<u8> {
    format!("agent-loop-state|{tenant}|{key}|{fence}").into_bytes()
}

fn encode<T: Serialize>(value: &T) -> Result<SolValue, ConversationStoreError> {
    let json = serde_json::to_value(value)
        .map_err(|error| ConversationStoreError::Corrupt(error.to_string()))?;
    aelio_kernel::json_from(&json)
        .map_err(|error| ConversationStoreError::Corrupt(error.to_string()))
}

fn decode<T: for<'de> Deserialize<'de>>(value: &SolValue) -> Result<T, ConversationStoreError> {
    serde_json::from_str(&aelio_sol::canonical_string(value))
        .map_err(|error| ConversationStoreError::Corrupt(error.to_string()))
}

fn store_error(error: StoreError) -> ConversationStoreError {
    ConversationStoreError::Store(format!("{error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_agent_loop::{EffectClass, ToolDefinition};
    use aelio_store::MemoryStore;
    use serde_json::json;
    use std::sync::{Arc, Barrier};

    fn state(secret: &str) -> ConversationState {
        let manifest = LoopManifest::admit(
            "tenant",
            "v1",
            vec![ToolDefinition {
                name: "lookup".to_string(),
                version: "1".to_string(),
                description: "Look up a customer-visible record".to_string(),
                input_schema: json!({"type":"object"}),
                effect_class: EffectClass::Read,
            }],
        )
        .unwrap();
        ConversationState {
            system: "system".to_string(),
            bootstrap: "bootstrap".to_string(),
            manifest,
            messages: vec![AgentMessage::User {
                text: secret.to_string(),
            }],
            pending_confirmation: None,
            pending_input: None,
            continuation_capabilities: Vec::new(),
            continuation_state_id: None,
            last_turn_id_hash: None,
            last_request_hash: None,
            compaction_count: 0,
            last_result: None,
            orchestration: None,
        }
    }

    #[test]
    fn encrypted_state_round_trips_without_plaintext_in_store() {
        let mut store = MemoryStore::new();
        let key = StateKey::test_key();
        let mut repo = ConversationRepository::new(&mut store, key.clone());
        let lease = repo.acquire("tenant", "user", "turn-1", 10, 100).unwrap();
        repo.commit(lease, &state("phone=9876543210")).unwrap();

        let row = store
            .get("tenant", TABLE, &digest("user"))
            .unwrap()
            .unwrap();
        assert!(!aelio_sol::canonical_string(&row.value).contains("9876543210"));

        let mut repo = ConversationRepository::new(&mut store, key);
        let lease = repo.acquire("tenant", "user", "turn-2", 200, 100).unwrap();
        assert!(matches!(
            &lease.state.unwrap().messages[0],
            AgentMessage::User { text } if text == "phone=9876543210"
        ));
    }

    #[test]
    fn state_key_accepts_documented_hex_and_base64url_encodings() {
        assert!(StateKey::parse(&"5a".repeat(32)).is_ok());
        assert!(StateKey::parse(&URL_SAFE_NO_PAD.encode([0x5a; 32])).is_ok());
        assert!(StateKey::parse("too-short").is_err());
    }

    #[test]
    fn stale_holder_cannot_commit_after_takeover() {
        let mut first_store = MemoryStore::new();
        let mut second_store = first_store.clone();
        let key = StateKey::test_key();
        let first = ConversationRepository::new(&mut first_store, key.clone())
            .acquire("tenant", "user", "turn-1", 0, 10)
            .unwrap();
        let second = ConversationRepository::new(&mut second_store, key.clone())
            .acquire("tenant", "user", "turn-2", 11, 10)
            .unwrap();
        assert!(matches!(
            ConversationRepository::new(&mut first_store, key.clone()).commit(first, &state("one")),
            Err(ConversationStoreError::LeaseLost)
        ));
        ConversationRepository::new(&mut second_store, key)
            .commit(second, &state("two"))
            .unwrap();
    }

    #[test]
    fn twenty_way_race_has_one_winner() {
        let store = MemoryStore::new();
        let barrier = Arc::new(Barrier::new(20));
        let winners = std::thread::scope(|scope| {
            let handles = (0..20)
                .map(|index| {
                    let mut store = store.clone();
                    let barrier = Arc::clone(&barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        ConversationRepository::new(&mut store, StateKey::test_key())
                            .acquire("tenant", "same-user", &format!("turn-{index}"), 0, 100)
                            .is_ok()
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|won| *won)
                .count()
        });
        assert_eq!(winners, 1);
    }

    #[test]
    fn all_rate_limit_scopes_are_reserved_and_block_before_dispatch() {
        let mut store = MemoryStore::new();
        let calls = vec![ToolCall {
            id: "call-1".to_string(),
            name: "orders.list".to_string(),
            arguments: json!({}),
        }];
        let limits = RateLimits {
            tenant_per_minute: 1,
            user_per_minute: 1,
            conversation_per_minute: 1,
            tool_per_minute: 1,
        };
        preflight_tool_batch_with_limits(
            &mut store,
            "tenant",
            "user",
            "conversation",
            &calls,
            60_000,
            limits,
        )
        .unwrap();
        assert_eq!(
            store
                .scan_prefix("tenant", RATE_LIMIT_TABLE, "", 10)
                .unwrap()
                .len(),
            4
        );
        assert!(matches!(
            preflight_tool_batch_with_limits(
                &mut store,
                "tenant",
                "user",
                "conversation",
                &calls,
                60_001,
                limits,
            ),
            Err(ConversationStoreError::RateLimited(scope)) if scope == "tenant"
        ));
        assert!(preflight_tool_batch_with_limits(
            &mut store,
            "tenant",
            "user",
            "conversation",
            &calls,
            120_000,
            limits,
        )
        .is_ok());
    }

    #[test]
    fn cancellation_marker_targets_only_the_active_fence_and_can_be_consumed() {
        let mut store = MemoryStore::new();
        let lease = ConversationRepository::new(&mut store, StateKey::test_key())
            .acquire("tenant", "user", "turn-1", 10, 100)
            .unwrap();
        assert_eq!(
            request_active_cancellation(&mut store, "tenant", "user", 20).unwrap(),
            Some(lease.fence)
        );
        assert!(cancellation_requested(&store, "tenant", "user", lease.fence, 21).unwrap());
        assert!(!cancellation_requested(&store, "tenant", "user", lease.fence + 1, 21).unwrap());
        clear_cancellation(&mut store, "tenant", "user", lease.fence, 22).unwrap();
        assert!(!cancellation_requested(&store, "tenant", "user", lease.fence, 23).unwrap());
    }

    #[test]
    fn missing_input_wait_state_survives_encrypted_restart() {
        let mut store = MemoryStore::new();
        let key = StateKey::test_key();
        let lease = ConversationRepository::new(&mut store, key.clone())
            .acquire("tenant", "user", "turn-1", 10, 100)
            .unwrap();
        let mut waiting = state("no secret");
        waiting.pending_input = Some(PendingInput {
            calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "lookup".to_string(),
                arguments: json!({}),
            }],
            missing_fields: vec!["order_id".to_string()],
            requested_at_ms: 10,
        });
        ConversationRepository::new(&mut store, key.clone())
            .commit(lease, &waiting)
            .unwrap();

        let resumed = ConversationRepository::new(&mut store, key)
            .acquire("tenant", "user", "turn-2", 200, 100)
            .unwrap()
            .state
            .unwrap();
        let pending = resumed.pending_input.expect("pending input resumes");
        assert_eq!(pending.missing_fields, vec!["order_id"]);
        assert_eq!(pending.calls[0].id, "call-1");
    }

    #[test]
    fn crash_replay_ignores_fresh_provider_ids_but_rejects_changed_effect() {
        let mut store = MemoryStore::new();
        let original = vec![(
            ToolCall {
                id: "provider-call-a".to_string(),
                name: "charge".to_string(),
                arguments: json!({"amount":100,"currency":"USD"}),
            },
            "1".to_string(),
        )];
        let first =
            reserve_invocation_batch(&mut store, "tenant", "conversation", "turn-1", 0, &original)
                .unwrap();
        let replay = vec![(
            ToolCall {
                id: "fresh-provider-id".to_string(),
                ..original[0].0.clone()
            },
            "1".to_string(),
        )];
        let second =
            reserve_invocation_batch(&mut store, "tenant", "conversation", "turn-1", 0, &replay)
                .unwrap();
        assert_eq!(first[0].1, second[0].1);
        assert_ne!(first[0].0, second[0].0);

        let altered = vec![(
            ToolCall {
                id: "third-provider-id".to_string(),
                name: "charge".to_string(),
                arguments: json!({"amount":101,"currency":"USD"}),
            },
            "1".to_string(),
        )];
        assert!(matches!(
            reserve_invocation_batch(&mut store, "tenant", "conversation", "turn-1", 0, &altered,),
            Err(ConversationStoreError::InvocationConflict(_))
        ));
    }
}
