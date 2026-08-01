//! Authenticated SDK catalog registration and correlated tool invocation bridge.

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

use aelio_agent::abilities::invoke::ToolHost;
use aelio_agent::runtime::durable::validate_catalog;
use aelio_agent::tenant::TenantDecl;
use aelio_agent::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc as tokio_mpsc;
use uuid::Uuid;

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
pub struct ResultMessage {
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
    completion: mpsc::SyncSender<AelioResult<Value>>,
}

impl SdkBridge {
    pub fn new(config: BridgeConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BridgeState {
                connections: HashMap::new(),
                catalogs: HashMap::new(),
                pending: HashMap::new(),
            })),
            config,
        }
    }

    pub fn open_connection(
        &self,
        connection_id: impl Into<String>,
    ) -> tokio_mpsc::Receiver<InvokeMessage> {
        let connection_id = connection_id.into();
        let (outbound, receiver) = tokio_mpsc::channel(self.config.outbound_capacity.max(1));
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
            fail_pending_for_connection(
                &mut state,
                replaced_id,
                "SDK connection was replaced by a reconnect",
            );
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
        fail_pending_for_connection(&mut state, connection_id, "SDK connection disconnected");
    }

    pub fn complete(&self, connection_id: &str, result: ResultMessage) -> AelioResult<()> {
        if result.invocation_id.trim().is_empty() {
            return Err(error(
                ReasonCode::Validation,
                "result invocation_id is required",
            ));
        }
        let pending = {
            let mut state = self.state();
            let pending = state
                .pending
                .remove(&result.invocation_id)
                .ok_or_else(|| error(ReasonCode::NotFound, "unknown or expired invocation_id"))?;
            if pending.connection_id != connection_id {
                state.pending.insert(result.invocation_id, pending);
                return Err(error(
                    ReasonCode::Denied,
                    "invocation result came from a different SDK connection",
                ));
            }
            pending
        };
        let completion = if result.ok {
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
        let _ = pending.completion.try_send(completion);
        Ok(())
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

    pub fn tool_host(&self, tenant_id: impl Into<String>) -> BridgeToolHost {
        BridgeToolHost {
            bridge: self.clone(),
            tenant_id: tenant_id.into(),
        }
    }

    pub fn invoke_versioned(
        &self,
        tenant_id: &str,
        tool_id: &str,
        tool_version: &str,
        args: &IndexMap<String, Value>,
    ) -> AelioResult<Value> {
        let invocation_id = Uuid::new_v4().to_string();
        let (completion, receiver) = mpsc::sync_channel(1);
        let outbound = {
            let mut state = self.state();
            let active = state
                .catalogs
                .get(tenant_id)
                .ok_or_else(|| error(ReasonCode::NotFound, "tenant catalog is not registered"))?;
            let tool = active
                .catalog
                .tools
                .iter()
                .find(|tool| tool.id == tool_id)
                .ok_or_else(|| error(ReasonCode::NotFound, "tool is not in the tenant catalog"))?;
            if tool.version != tool_version {
                return Err(error(
                    ReasonCode::Conflict,
                    "requested tool version does not match the active catalog",
                ));
            }
            let connection_id = active
                .connection_id
                .clone()
                .ok_or_else(|| error(ReasonCode::Unavailable, "tenant tool host is offline"))?;
            let tool_version = tool.version.clone();
            let in_flight = state
                .pending
                .values()
                .filter(|pending| pending.tenant_id == tenant_id)
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
            state.pending.insert(
                invocation_id.clone(),
                PendingInvocation {
                    tenant_id: tenant_id.to_owned(),
                    connection_id,
                    completion,
                },
            );
            (
                sender,
                InvokeMessage {
                    message_type: "invoke",
                    id: invocation_id.clone(),
                    invocation_id: invocation_id.clone(),
                    tenant_id: tenant_id.to_owned(),
                    tool_id: tool_id.to_owned(),
                    tool_version,
                    function: tool_id.to_owned(),
                    args: args.clone(),
                },
            )
        };

        if outbound.0.try_send(outbound.1).is_err() {
            self.state().pending.remove(&invocation_id);
            return Err(error(
                ReasonCode::Unavailable,
                "SDK outbound queue is unavailable or full",
            ));
        }
        match receiver.recv_timeout(self.config.invocation_timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.state().pending.remove(&invocation_id);
                Err(error(ReasonCode::Timeout, "SDK tool invocation timed out"))
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
        self.bridge
            .invoke_versioned(&self.tenant_id, tool_id, &version, args)
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

fn fail_pending_for_connection(state: &mut BridgeState, connection_id: &str, message: &str) {
    let invocation_ids: Vec<_> = state
        .pending
        .iter()
        .filter(|(_, pending)| pending.connection_id == connection_id)
        .map(|(id, _)| id.clone())
        .collect();
    for invocation_id in invocation_ids {
        if let Some(pending) = state.pending.remove(&invocation_id) {
            let _ = pending
                .completion
                .try_send(Err(error(ReasonCode::Unavailable, message)));
        }
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
        assert_eq!(first.join().unwrap().unwrap_err().code, ReasonCode::Timeout);
        let second = {
            let bridge = bridge.clone();
            std::thread::spawn(move || {
                bridge.invoke_versioned("tenant-a", "send_otp", "1", &IndexMap::new())
            })
        };
        outbound.recv().await.unwrap();
        assert_eq!(
            second.join().unwrap().unwrap_err().code,
            ReasonCode::Timeout
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
                    invocation_id: request.invocation_id,
                    ok: true,
                    data: Some(Value::Bool(true)),
                    error: None,
                },
            )
            .unwrap();
        assert!(invoking.join().unwrap().is_ok());
    }
}
