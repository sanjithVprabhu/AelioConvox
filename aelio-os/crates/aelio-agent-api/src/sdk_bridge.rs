//! Authenticated SDK catalog registration and correlated tool invocation bridge.

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aelio_agent::abilities::invoke::ToolHost;
use aelio_agent::runtime::durable::validate_catalog;
use aelio_agent::tenant::TenantDecl;
use aelio_agent::{AelioError, AelioResult, ReasonCode, Value};
use aelio_sol::SolValue;
use aelio_wire::{DeliveryBook, ExpiryDisposition, ResultDisposition, ServerFrame};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc as tokio_mpsc;
use uuid::Uuid;

const TERMINAL_CORRELATION_RETENTION_MS: u64 = 5 * 60 * 1_000;
const MAX_TERMINAL_CORRELATIONS: usize = 4_096;

#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub invocation_timeout: Duration,
    pub max_in_flight_per_tenant: usize,
    pub outbound_capacity: usize,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            invocation_timeout: Duration::from_secs(30),
            max_in_flight_per_tenant: 64,
            outbound_capacity: 128,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistrationDiagnostic {
    pub level: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAck {
    #[serde(rename = "type")]
    pub message_type: String,
    pub status: String,
    pub tenant_id: String,
    pub revision: u64,
    pub catalog_hash: String,
    pub available: bool,
    pub diagnostics: Vec<RegistrationDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvokeMessage {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    /// Compatibility alias for SDKs which correlate results using `id`.
    pub id: String,
    pub invocation_id: String,
    pub tenant_id: String,
    pub tool_id: String,
    pub tool_version: String,
    /// Compatibility alias for the existing SDK function field.
    pub function: String,
    pub args: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    #[serde(default, alias = "id")]
    pub invocation_id: String,
    pub ok: bool,
    pub data: Option<Value>,
    pub error: Option<ResultError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResultError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Debug, Clone)]
pub struct CatalogStatus {
    pub tenant_id: String,
    pub revision: u64,
    pub catalog_hash: String,
    pub available: bool,
    pub catalog: TenantDecl,
}

#[derive(Clone)]
pub struct SdkBridge {
    inner: Arc<Mutex<BridgeState>>,
    config: BridgeConfig,
}

struct BridgeState {
    connections: HashMap<String, Connection>,
    catalogs: HashMap<String, ActiveCatalog>,
    pending: HashMap<String, PendingInvocation>,
    delivery: DeliveryBook,
}

struct Connection {
    outbound: tokio_mpsc::Sender<InvokeMessage>,
    tenant_id: Option<String>,
}

struct ActiveCatalog {
    catalog: TenantDecl,
    revision: u64,
    catalog_hash: String,
    connection_id: Option<String>,
}

struct PendingInvocation {
    tenant_id: String,
    connection_id: String,
    completion: Option<mpsc::SyncSender<AelioResult<Value>>>,
    message: InvokeMessage,
    deadline_at_ms: u64,
    terminal_at_ms: Option<u64>,
    events: Vec<&'static str>,
}

#[derive(Serialize)]
struct SdkDeliveryTrace<'a> {
    authority: &'static str,
    correlation: String,
    steps: Vec<SdkDeliveryTraceStep<'a>>,
}

#[derive(Serialize)]
struct SdkDeliveryTraceStep<'a> {
    seq: usize,
    kind: &'a str,
}

impl SdkBridge {
    pub fn new(config: BridgeConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BridgeState {
                connections: HashMap::new(),
                catalogs: HashMap::new(),
                pending: HashMap::new(),
                delivery: DeliveryBook::default(),
            })),
            config,
        }
    }

    pub fn open_connection(
        &self,
        connection_id: impl Into<String>,
    ) -> tokio_mpsc::Receiver<InvokeMessage> {
        let connection_id = connection_id.into();
        let (outbound, receiver) = tokio_mpsc::channel(
            self.config
                .outbound_capacity
                .max(self.config.max_in_flight_per_tenant)
                .max(1),
        );
        self.state().connections.insert(
            connection_id,
            Connection {
                outbound,
                tenant_id: None,
            },
        );
        receiver
    }

    /// Validates and atomically publishes a complete catalog for an authenticated tenant.
    pub fn register(
        &self,
        connection_id: &str,
        authenticated_tenant: &str,
        catalog: TenantDecl,
    ) -> AelioResult<RegisterAck> {
        if authenticated_tenant.trim().is_empty() {
            return Err(error(ReasonCode::Denied, "authenticated tenant is missing"));
        }
        if catalog.tenant_id != authenticated_tenant {
            return Err(error(
                ReasonCode::Denied,
                "catalog tenant does not match authenticated credentials",
            ));
        }
        validate_catalog(&catalog)?;
        let catalog_hash = canonical_catalog_hash(&catalog)?;

        let mut state = self.state();
        if !state.connections.contains_key(connection_id) {
            return Err(error(ReasonCode::Unavailable, "SDK connection is closed"));
        }

        let replaced = state
            .catalogs
            .get(authenticated_tenant)
            .and_then(|active| active.connection_id.clone())
            .filter(|active_id| active_id != connection_id);
        if let Some(replaced_id) = replaced.as_deref() {
            if let Some(connection) = state.connections.get_mut(replaced_id) {
                connection.tenant_id = None;
            }
        }

        let revision = state
            .catalogs
            .get(authenticated_tenant)
            .map_or(1, |active| active.revision.saturating_add(1));
        state.catalogs.insert(
            authenticated_tenant.to_owned(),
            ActiveCatalog {
                catalog,
                revision,
                catalog_hash: catalog_hash.clone(),
                connection_id: Some(connection_id.to_owned()),
            },
        );
        if let Some(connection) = state.connections.get_mut(connection_id) {
            connection.tenant_id = Some(authenticated_tenant.to_owned());
        }

        // At-least-once transport delivery reuses the original correlation identity. The SDK is
        // responsible for attaching to an in-flight call or replaying its cached result; the
        // server never creates a second logical invocation during reconnect.
        let sender = state
            .connections
            .get(connection_id)
            .map(|connection| connection.outbound.clone())
            .ok_or_else(|| error(ReasonCode::Unavailable, "SDK connection is closed"))?;
        let now_ms = unix_time_ms();
        let unresolved = state
            .pending
            .values_mut()
            .filter(|pending| {
                pending.tenant_id == authenticated_tenant
                    && pending.completion.is_some()
                    && now_ms < pending.deadline_at_ms
            })
            .map(|pending| {
                pending.connection_id = connection_id.to_owned();
                pending.message.clone()
            })
            .collect::<Vec<_>>();
        for message in unresolved {
            sender.try_send(message).map_err(|_| {
                error(
                    ReasonCode::Unavailable,
                    "SDK reconnect queue could not accept unresolved invocations",
                )
            })?;
        }

        Ok(RegisterAck {
            message_type: "register_ack".to_owned(),
            status: "active".to_owned(),
            tenant_id: authenticated_tenant.to_owned(),
            revision,
            catalog_hash,
            available: true,
            diagnostics: vec![RegistrationDiagnostic {
                level: "info".to_owned(),
                code: "catalog_activated".to_owned(),
                message: "complete catalog snapshot validated and activated".to_owned(),
            }],
        })
    }

    pub fn disconnect(&self, connection_id: &str) {
        let mut state = self.state();
        let tenant_id = state
            .connections
            .remove(connection_id)
            .and_then(|connection| connection.tenant_id);
        if let Some(tenant_id) = tenant_id {
            if let Some(catalog) = state.catalogs.get_mut(&tenant_id) {
                if catalog.connection_id.as_deref() == Some(connection_id) {
                    catalog.connection_id = None;
                }
            }
        }
        // Unresolved calls remain correlated until their authoritative deadline. A reconnect may
        // resume their delivery with the same corr; expiry classifies prepared versus dispatched.
    }

    /// Mark the exact socket write boundary. Re-delivery after reconnect is idempotent and keeps
    /// the original correlation id; a foreign or stale connection cannot advance the record.
    pub fn mark_dispatched(&self, connection_id: &str, invocation_id: &str) -> AelioResult<()> {
        let mut state = self.state();
        let pending = state
            .pending
            .get(invocation_id)
            .ok_or_else(|| error(ReasonCode::NotFound, "unknown invocation_id"))?;
        if pending.connection_id != connection_id {
            return Err(error(
                ReasonCode::Denied,
                "invocation was dispatched by a stale SDK connection",
            ));
        }
        state
            .delivery
            .mark_dispatched(invocation_id)
            .map_err(|message| error(ReasonCode::Conflict, message))?;
        if let Some(pending) = state.pending.get_mut(invocation_id) {
            if !pending.events.contains(&"call_dispatch") {
                pending.events.push("call_dispatch");
            }
        }
        Ok(())
    }

    pub fn complete(
        &self,
        connection_id: &str,
        result: ResultMessage,
    ) -> AelioResult<ResultDisposition> {
        if result.message_type != "result"
            || result.invocation_id.trim().is_empty()
            || result.invocation_id.len() > 256
            || (result.ok && (result.data.is_none() || result.error.is_some()))
            || (!result.ok && (result.data.is_some() || result.error.is_none()))
            || result.error.as_ref().is_some_and(|error| {
                error.code.trim().is_empty()
                    || error.code.len() > 128
                    || error.message.trim().is_empty()
                    || error.message.len() > 2_048
            })
        {
            return Err(error(
                ReasonCode::Validation,
                "result must have a bounded correlation and exactly one valid success/error body",
            ));
        }
        let completion_value = if result.ok {
            Ok(result.data.unwrap_or(Value::Null))
        } else {
            let result_error = result.error.unwrap_or(ResultError {
                code: "tool_error".to_owned(),
                message: "SDK tool invocation failed".to_owned(),
                retryable: false,
            });
            let code = match result_error.code.as_str() {
                "timeout" => ReasonCode::Timeout,
                "rate_limited" => ReasonCode::RateLimited,
                "unavailable" => ReasonCode::Unavailable,
                _ => ReasonCode::ToolError,
            };
            Err(error(code, result_error.message))
        };
        let invocation_id = result.invocation_id;
        let (disposition, completion) = {
            let mut state = self.state();
            prune_terminal_correlations(&mut state, unix_time_ms());
            let pending = state
                .pending
                .get(&invocation_id)
                .ok_or_else(|| error(ReasonCode::NotFound, "unknown or expired invocation_id"))?;
            let active_connection = state
                .catalogs
                .get(&pending.tenant_id)
                .and_then(|catalog| catalog.connection_id.as_deref());
            if active_connection != Some(connection_id) {
                return Err(error(
                    ReasonCode::Denied,
                    "invocation result did not come from the tenant's active SDK connection",
                ));
            }
            let now_ms = unix_time_ms();
            if now_ms >= pending.deadline_at_ms {
                let _ = state.delivery.expire(&invocation_id, now_ms);
                if let Some(pending) = state.pending.get_mut(&invocation_id) {
                    pending.completion = None;
                    pending.terminal_at_ms.get_or_insert(now_ms);
                }
            }
            let disposition = state.delivery.accept_result(&invocation_id);
            if let Some(pending) = state.pending.get_mut(&invocation_id) {
                pending.events.push(match disposition {
                    ResultDisposition::Accepted => "call_result",
                    ResultDisposition::Duplicate => "duplicate_result",
                    ResultDisposition::Late => "late_result",
                    ResultDisposition::UnknownCorrelation => "unknown_result",
                });
            }
            let completion = match disposition {
                ResultDisposition::Accepted => {
                    let pending = state.pending.get_mut(&invocation_id).ok_or_else(|| {
                        error(ReasonCode::Internal, "delivery correlation lost its owner")
                    })?;
                    pending.terminal_at_ms = Some(now_ms);
                    pending.completion.take()
                }
                ResultDisposition::Duplicate | ResultDisposition::Late => None,
                ResultDisposition::UnknownCorrelation => {
                    return Err(error(
                        ReasonCode::Internal,
                        "delivery book lost a live bridge correlation",
                    ));
                }
            };
            (disposition, completion)
        };
        if let Some(completion) = completion {
            let _ = completion.try_send(completion_value);
        }
        Ok(disposition)
    }

    pub fn catalog_status(&self, tenant_id: &str) -> Option<CatalogStatus> {
        self.state()
            .catalogs
            .get(tenant_id)
            .map(|active| CatalogStatus {
                tenant_id: tenant_id.to_owned(),
                revision: active.revision,
                catalog_hash: active.catalog_hash.clone(),
                available: active.connection_id.is_some(),
                catalog: active.catalog.clone(),
            })
    }

    pub fn delivery_trace(&self, invocation_id: &str) -> AelioResult<String> {
        let state = self.state();
        let pending = state
            .pending
            .get(invocation_id)
            .ok_or_else(|| error(ReasonCode::NotFound, "delivery trace was not retained"))?;
        let correlation = hex::encode(Sha256::digest(invocation_id.as_bytes()));
        serde_json::to_string(&SdkDeliveryTrace {
            authority: "aelio-wire",
            correlation: format!("sha256:{}", &correlation[..16]),
            steps: pending
                .events
                .iter()
                .enumerate()
                .map(|(seq, kind)| SdkDeliveryTraceStep { seq, kind })
                .collect(),
        })
        .map_err(|cause| error(ReasonCode::Internal, cause.to_string()))
    }

    pub fn tool_host(&self, tenant_id: impl Into<String>) -> BridgeToolHost {
        BridgeToolHost {
            bridge: self.clone(),
            tenant_id: tenant_id.into(),
            last_trace: None,
        }
    }

    pub fn invoke_versioned(
        &self,
        tenant_id: &str,
        tool_id: &str,
        tool_version: &str,
        args: &IndexMap<String, Value>,
    ) -> AelioResult<Value> {
        self.invoke_versioned_traced(tenant_id, tool_id, tool_version, args)
            .map(|(value, _)| value)
    }

    fn invoke_versioned_traced(
        &self,
        tenant_id: &str,
        tool_id: &str,
        tool_version: &str,
        args: &IndexMap<String, Value>,
    ) -> AelioResult<(Value, String)> {
        let invocation_id = Uuid::new_v4().to_string();
        let (completion, receiver) = mpsc::sync_channel(1);
        let timeout_ms = u64::try_from(self.config.invocation_timeout.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        let deadline_at_ms = unix_time_ms().saturating_add(timeout_ms);
        let outbound = {
            let mut state = self.state();
            prune_terminal_correlations(&mut state, unix_time_ms());
            let (function_name, connection_id, tool_version, effect_class) = {
                let active = state.catalogs.get(tenant_id).ok_or_else(|| {
                    error(ReasonCode::NotFound, "tenant catalog is not registered")
                })?;
                let tool = active
                    .catalog
                    .tools
                    .iter()
                    .find(|tool| tool.id == tool_id)
                    .ok_or_else(|| {
                        error(ReasonCode::NotFound, "tool is not in the tenant catalog")
                    })?;
                if tool.version != tool_version {
                    return Err(error(
                        ReasonCode::Conflict,
                        "requested tool version does not match the active catalog",
                    ));
                }
                (
                    tool.name.clone(),
                    active.connection_id.clone().ok_or_else(|| {
                        error(ReasonCode::Unavailable, "tenant tool host is offline")
                    })?,
                    tool.version.clone(),
                    match tool.effect_class() {
                        aelio_agent::tenant::ToolEffect::Pure => "pure",
                        aelio_agent::tenant::ToolEffect::Read => "read",
                        aelio_agent::tenant::ToolEffect::Write => "write",
                        aelio_agent::tenant::ToolEffect::External => "external",
                    },
                )
            };
            let in_flight = state
                .pending
                .values()
                .filter(|pending| pending.tenant_id == tenant_id && pending.completion.is_some())
                .count();
            if in_flight >= self.config.max_in_flight_per_tenant {
                return Err(error(
                    ReasonCode::RateLimited,
                    "tenant SDK invocation limit reached",
                ));
            }
            let sender = state
                .connections
                .get(&connection_id)
                .map(|connection| connection.outbound.clone())
                .ok_or_else(|| error(ReasonCode::Unavailable, "tenant tool host is offline"))?;
            let message = InvokeMessage {
                message_type: "invoke",
                id: invocation_id.clone(),
                invocation_id: invocation_id.clone(),
                tenant_id: tenant_id.to_owned(),
                tool_id: tool_id.to_owned(),
                tool_version: tool_version.clone(),
                // `tool_id` is the closed DSL/kernel identifier. `name` is the exact external
                // function exported by the SDK and may use the host language's conventions.
                function: function_name,
                args: args.clone(),
            };
            state
                .delivery
                .prepare(
                    ServerFrame::ToolCall {
                        corr: invocation_id.clone(),
                        target: format!("{tool_id}@{tool_version}"),
                        effect_class: effect_class.into(),
                        args: values_to_sol(args)?,
                        deadline_ms: timeout_ms,
                        idem_key: None,
                        turn_ref: format!("sdk:{invocation_id}"),
                    },
                    deadline_at_ms,
                )
                .map_err(|message| error(ReasonCode::Conflict, message))?;
            state.pending.insert(
                invocation_id.clone(),
                PendingInvocation {
                    tenant_id: tenant_id.to_owned(),
                    connection_id,
                    completion: Some(completion),
                    message: message.clone(),
                    deadline_at_ms,
                    terminal_at_ms: None,
                    events: vec!["call_intent"],
                },
            );
            (sender, message)
        };

        if outbound.0.try_send(outbound.1).is_err() {
            let mut state = self.state();
            state.pending.remove(&invocation_id);
            let _ = state.delivery.cancel_prepared(&invocation_id);
            return Err(error(
                ReasonCode::Unavailable,
                "SDK outbound queue is unavailable or full",
            ));
        }
        match receiver.recv_timeout(self.config.invocation_timeout) {
            Ok(result) => {
                let trace = self.delivery_trace(&invocation_id)?;
                result
                    .map(|value| (value, trace.clone()))
                    .map_err(|error| error.with_decision_trace(Some(trace)))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let disposition = {
                    let mut state = self.state();
                    let now_ms = unix_time_ms().max(deadline_at_ms);
                    let disposition = state
                        .delivery
                        .expire(&invocation_id, now_ms)
                        .map_err(|message| error(ReasonCode::Internal, message))?
                        .ok_or_else(|| {
                            error(ReasonCode::Internal, "delivery deadline did not expire")
                        })?;
                    if let Some(pending) = state.pending.get_mut(&invocation_id) {
                        pending.completion = None;
                        pending.terminal_at_ms = Some(now_ms);
                    }
                    disposition
                };
                let trace = self.delivery_trace(&invocation_id).ok();
                match disposition {
                    ExpiryDisposition::ToolTransient => Err(error(
                        ReasonCode::Unavailable,
                        "SDK call expired before a result and is safe to retry",
                    )
                    .with_decision_trace(trace)),
                    ExpiryDisposition::InternalUnknownOutcome => Err(error(
                        ReasonCode::Internal,
                        "SDK effect was dispatched but its outcome is unknown",
                    )
                    .with_detail(Value::Map(indexmap::indexmap! {
                        "outcome".into() => Value::str("unknown_outcome"),
                        "boundary".into() => Value::str("sdk_dispatch"),
                    }))
                    .with_decision_trace(trace)),
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(error(
                ReasonCode::Unavailable,
                "SDK tool invocation was disconnected",
            )),
        }
    }

    fn state(&self) -> MutexGuard<'_, BridgeState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub struct BridgeToolHost {
    bridge: SdkBridge,
    tenant_id: String,
    last_trace: Option<String>,
}

impl ToolHost for BridgeToolHost {
    fn call(&mut self, tool_id: &str, args: &IndexMap<String, Value>) -> AelioResult<Value> {
        let version = self
            .bridge
            .catalog_status(&self.tenant_id)
            .and_then(|status| {
                status
                    .catalog
                    .tools
                    .into_iter()
                    .find(|tool| tool.id == tool_id)
                    .map(|tool| tool.version)
            })
            .ok_or_else(|| error(ReasonCode::NotFound, "tool is not in the tenant catalog"))?;
        match self
            .bridge
            .invoke_versioned_traced(&self.tenant_id, tool_id, &version, args)
        {
            Ok((value, trace)) => {
                self.last_trace = Some(trace);
                Ok(value)
            }
            Err(error) => {
                self.last_trace = error.decision_trace.as_deref().map(str::to_owned);
                Err(error)
            }
        }
    }

    fn take_decision_trace(&mut self) -> Option<String> {
        self.last_trace.take()
    }
}

fn canonical_catalog_hash(catalog: &TenantDecl) -> AelioResult<String> {
    let encoded = serde_json::to_vec(catalog).map_err(|cause| {
        error(
            ReasonCode::Validation,
            format!("catalog serialization failed: {cause}"),
        )
    })?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn prune_terminal_correlations(state: &mut BridgeState, now_ms: u64) {
    let mut terminal = state
        .pending
        .iter()
        .filter_map(|(id, pending)| pending.terminal_at_ms.map(|at| (id.clone(), at)))
        .collect::<Vec<_>>();
    terminal.sort_by_key(|(_, at)| *at);
    let excess = terminal.len().saturating_sub(MAX_TERMINAL_CORRELATIONS);
    for (index, (invocation_id, terminal_at_ms)) in terminal.into_iter().enumerate() {
        if (index < excess
            || now_ms >= terminal_at_ms.saturating_add(TERMINAL_CORRELATION_RETENTION_MS))
            && state.delivery.remove_terminal(&invocation_id).is_ok()
        {
            state.pending.remove(&invocation_id);
        }
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn values_to_sol(values: &IndexMap<String, Value>) -> AelioResult<SolValue> {
    values
        .iter()
        .map(|(name, value)| Ok((name.clone(), value_to_sol(value)?)))
        .collect::<AelioResult<Vec<_>>>()
        .map(SolValue::map)
}

fn value_to_sol(value: &Value) -> AelioResult<SolValue> {
    match value {
        Value::Null => Ok(SolValue::Null),
        Value::Bool(value) => Ok(SolValue::Bool(*value)),
        Value::Int(value) => Ok(SolValue::Int(*value)),
        Value::Float(value) => SolValue::float(*value)
            .map_err(|cause| error(ReasonCode::Validation, cause.to_string())),
        Value::Str(value) => Ok(SolValue::str(value)),
        Value::List(values) => values
            .iter()
            .map(value_to_sol)
            .collect::<AelioResult<Vec<_>>>()
            .map(SolValue::List),
        Value::Map(values) => values_to_sol(values),
    }
}

fn error(code: ReasonCode, message: impl Into<String>) -> AelioError {
    AelioError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_agent::runtime::World;

    fn catalog(tenant_id: &str) -> TenantDecl {
        World::demo_tenant(tenant_id).tenant
    }

    fn bridge(timeout: Duration, max_in_flight: usize) -> SdkBridge {
        SdkBridge::new(BridgeConfig {
            invocation_timeout: timeout,
            max_in_flight_per_tenant: max_in_flight,
            outbound_capacity: 8,
        })
    }

    #[tokio::test]
    async fn registration_validates_and_activates_complete_snapshot_atomically() {
        let bridge = bridge(Duration::from_secs(1), 2);
        let _outbound = bridge.open_connection("c1");
        let ack = bridge
            .register("c1", "tenant-a", catalog("tenant-a"))
            .unwrap();
        assert_eq!(ack.revision, 1);
        assert_eq!(ack.catalog_hash.len(), 64);
        assert!(!ack.diagnostics.is_empty());

        let mut invalid = catalog("tenant-a");
        invalid.tools.push(invalid.tools[0].clone());
        assert_eq!(
            bridge.register("c1", "tenant-a", invalid).unwrap_err().code,
            ReasonCode::Conflict
        );
        let active = bridge.catalog_status("tenant-a").unwrap();
        assert_eq!(active.revision, 1);
        assert_eq!(active.catalog_hash, ack.catalog_hash);
    }

    #[tokio::test]
    async fn invocation_carries_identity_version_and_correlates_result() {
        let bridge = bridge(Duration::from_secs(1), 2);
        let mut outbound = bridge.open_connection("c1");
        bridge
            .register("c1", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let invoking = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned(
                    "tenant-a",
                    "send_otp",
                    "1",
                    &indexmap::indexmap! {"phone".into() => Value::str("+15551234567")},
                )
            })
        };
        let request = outbound.recv().await.unwrap();
        assert_eq!(request.tenant_id, "tenant-a");
        assert_eq!(request.tool_id, "send_otp");
        assert_eq!(request.tool_version, "1");
        assert_eq!(request.id, request.invocation_id);
        bridge
            .complete(
                "c1",
                ResultMessage {
                    message_type: "result".into(),
                    invocation_id: request.invocation_id,
                    ok: true,
                    data: Some(Value::Bool(true)),
                    error: None,
                },
            )
            .unwrap();
        assert_eq!(invoking.join().unwrap().unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn disconnect_preserves_catalog_but_marks_host_unavailable() {
        let bridge = bridge(Duration::from_secs(2), 2);
        let mut outbound = bridge.open_connection("c1");
        bridge
            .register("c1", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let invoking = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        outbound.recv().await.unwrap();
        bridge.disconnect("c1");
        assert!(!bridge.catalog_status("tenant-a").unwrap().available);
        assert_eq!(
            invoking.join().unwrap().unwrap_err().code,
            ReasonCode::Unavailable
        );
        assert_eq!(
            bridge
                .invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
                .unwrap_err()
                .code,
            ReasonCode::Unavailable
        );
    }

    #[tokio::test]
    async fn wrong_version_is_rejected_before_dispatch() {
        let bridge = bridge(Duration::from_secs(1), 2);
        let mut outbound = bridge.open_connection("c1");
        bridge
            .register("c1", "tenant-a", catalog("tenant-a"))
            .unwrap();
        assert_eq!(
            bridge
                .invoke_versioned("tenant-a", "send_otp", "9", &IndexMap::new())
                .unwrap_err()
                .code,
            ReasonCode::Conflict
        );
        assert!(outbound.try_recv().is_err());
    }

    #[tokio::test]
    async fn invocation_timeout_releases_bounded_in_flight_slot() {
        let bridge = bridge(Duration::from_millis(30), 1);
        let mut outbound = bridge.open_connection("c1");
        bridge
            .register("c1", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let first = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        outbound.recv().await.unwrap();
        assert_eq!(
            bridge
                .invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
                .unwrap_err()
                .code,
            ReasonCode::RateLimited
        );
        assert_eq!(
            first.join().unwrap().unwrap_err().code,
            ReasonCode::Unavailable
        );
        let second = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        outbound.recv().await.unwrap();
        assert_eq!(
            second.join().unwrap().unwrap_err().code,
            ReasonCode::Unavailable
        );
    }

    #[tokio::test]
    async fn results_and_lookups_are_tenant_and_connection_isolated() {
        let bridge = bridge(Duration::from_secs(1), 2);
        let mut outbound_a = bridge.open_connection("a");
        let _outbound_b = bridge.open_connection("b");
        bridge
            .register("a", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let mut tenant_b = catalog("tenant-b");
        tenant_b.tools.retain(|tool| tool.id != "send_otp");
        // Registration now proves every flow capability against a current tool, so dropping the
        // send_otp tool means the login flow that consumes `auth.otp.send` must go too.
        tenant_b.flows.retain(|flow| {
            !flow.steps.iter().any(|step| {
                step.admissible
                    .iter()
                    .any(|cap| cap.starts_with("auth.otp"))
            })
        });
        bridge.register("b", "tenant-b", tenant_b).unwrap();
        assert_eq!(
            bridge
                .invoke_versioned("tenant-b", "send_otp", "1", &IndexMap::new())
                .unwrap_err()
                .code,
            ReasonCode::NotFound
        );

        let invoking = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        let request = outbound_a.recv().await.unwrap();
        let foreign = ResultMessage {
            message_type: "result".into(),
            invocation_id: request.invocation_id.clone(),
            ok: true,
            data: Some(Value::Bool(false)),
            error: None,
        };
        assert_eq!(
            bridge.complete("b", foreign).unwrap_err().code,
            ReasonCode::Denied
        );
        bridge
            .complete(
                "a",
                ResultMessage {
                    message_type: "result".into(),
                    invocation_id: request.invocation_id,
                    ok: true,
                    data: Some(Value::Bool(true)),
                    error: None,
                },
            )
            .unwrap();
        assert_eq!(invoking.join().unwrap().unwrap(), Value::Bool(true));
    }

    #[tokio::test]
    async fn reconnect_replaces_handler_without_erasing_catalog_identity() {
        let bridge = bridge(Duration::from_secs(1), 2);
        let _old = bridge.open_connection("old");
        let mut new = bridge.open_connection("new");
        bridge
            .register("old", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let ack = bridge
            .register("new", "tenant-a", catalog("tenant-a"))
            .unwrap();
        assert_eq!(ack.revision, 2);
        bridge.disconnect("old");
        assert!(bridge.catalog_status("tenant-a").unwrap().available);

        let invoking = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        let request = new.recv().await.unwrap();
        bridge
            .complete(
                "new",
                ResultMessage {
                    message_type: "result".into(),
                    invocation_id: request.invocation_id,
                    ok: true,
                    data: Some(Value::Bool(true)),
                    error: None,
                },
            )
            .unwrap();
        assert!(invoking.join().unwrap().is_ok());
    }

    #[tokio::test]
    async fn duplicate_result_is_acked_and_discarded_without_second_completion() {
        let bridge = bridge(Duration::from_secs(1), 2);
        let mut outbound = bridge.open_connection("c1");
        bridge
            .register("c1", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let invoking = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        let request = outbound.recv().await.unwrap();
        bridge
            .mark_dispatched("c1", &request.invocation_id)
            .unwrap();
        assert_eq!(
            bridge
                .complete(
                    "c1",
                    ResultMessage {
                        message_type: "result".into(),
                        invocation_id: request.invocation_id.clone(),
                        ok: true,
                        data: Some(Value::Bool(true)),
                        error: None,
                    },
                )
                .unwrap(),
            ResultDisposition::Accepted
        );
        assert_eq!(invoking.join().unwrap().unwrap(), Value::Bool(true));
        assert_eq!(
            bridge
                .complete(
                    "c1",
                    ResultMessage {
                        message_type: "result".into(),
                        invocation_id: request.invocation_id,
                        ok: true,
                        data: Some(Value::Bool(false)),
                        error: None,
                    },
                )
                .unwrap(),
            ResultDisposition::Duplicate
        );
    }

    #[tokio::test]
    async fn dispatched_external_timeout_is_unknown_and_late_result_cannot_reenter() {
        let bridge = bridge(Duration::from_millis(30), 1);
        let mut outbound = bridge.open_connection("c1");
        bridge
            .register("c1", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let invoking = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        let request = outbound.recv().await.unwrap();
        bridge
            .mark_dispatched("c1", &request.invocation_id)
            .unwrap();
        let timeout = invoking.join().unwrap().unwrap_err();
        assert_eq!(timeout.code, ReasonCode::Internal);
        let trace: serde_json::Value = serde_json::from_str(
            timeout
                .decision_trace
                .as_deref()
                .expect("post-dispatch failures retain a metadata-only delivery trace"),
        )
        .unwrap();
        assert_eq!(
            trace["steps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|step| step["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["call_intent", "call_dispatch"]
        );
        assert_eq!(
            bridge
                .complete(
                    "c1",
                    ResultMessage {
                        message_type: "result".into(),
                        invocation_id: request.invocation_id,
                        ok: true,
                        data: Some(Value::Bool(true)),
                        error: None,
                    },
                )
                .unwrap(),
            ResultDisposition::Late
        );
    }

    #[tokio::test]
    async fn reconnect_redelivers_same_correlation_and_completes_original_waiter() {
        let bridge = bridge(Duration::from_secs(1), 2);
        let mut old = bridge.open_connection("old");
        bridge
            .register("old", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let invoking = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        let first = old.recv().await.unwrap();
        bridge.mark_dispatched("old", &first.invocation_id).unwrap();
        bridge.disconnect("old");

        let mut new = bridge.open_connection("new");
        bridge
            .register("new", "tenant-a", catalog("tenant-a"))
            .unwrap();
        let redelivered = new.recv().await.unwrap();
        assert_eq!(redelivered.invocation_id, first.invocation_id);
        bridge
            .mark_dispatched("new", &redelivered.invocation_id)
            .unwrap();
        assert_eq!(
            bridge
                .complete(
                    "new",
                    ResultMessage {
                        message_type: "result".into(),
                        invocation_id: redelivered.invocation_id,
                        ok: true,
                        data: Some(Value::Bool(true)),
                        error: None,
                    },
                )
                .unwrap(),
            ResultDisposition::Accepted
        );
        assert_eq!(invoking.join().unwrap().unwrap(), Value::Bool(true));
    }
}
