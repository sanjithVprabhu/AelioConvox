//! Adapter from the adaptive agent's `CapabilityHost` boundary into the authoritative artifact runtime.
//!
//! The agent is allowed to select a pinned capability. It is deliberately not allowed to execute
//! that capability directly: execution enters the corresponding admitted proxy Flow, where
//! policy, boundedness, idempotency, the effect ledger and host dispatch are enforced.

use aelio_agent::abilities::invoke::CapabilityHost;
use aelio_agent::tenant::ToolSpec;
use aelio_agent::{AelioError, AelioResult, ReasonCode, Value};
use aelio_runtime::{Runtime, RuntimeError, TurnReply, TurnSubmit};
use indexmap::IndexMap;
use sha2::{Digest, Sha256};

pub(crate) struct RuntimeArtifactToolHost {
    runtime: Runtime,
    tenant_id: String,
    last_trace: Option<String>,
}

impl RuntimeArtifactToolHost {
    pub(crate) fn new(runtime: Runtime, tenant_id: String) -> Self {
        Self {
            runtime,
            tenant_id,
            last_trace: None,
        }
    }
}

impl CapabilityHost for RuntimeArtifactToolHost {
    fn call_with_context(
        &mut self,
        tool: &ToolSpec,
        args: &IndexMap<String, Value>,
        idempotency_key: &str,
        _user_id: &str,
        channel: &str,
    ) -> AelioResult<Value> {
        let version = parse_version(tool)?;
        let flow_id = format!("aelio.proxy.{}", tool.id);
        let instance_id = stable_instance_id(&self.tenant_id, &tool.id, version, idempotency_key);
        let input = serde_json::json!({
            "args": args
                .iter()
                .map(|(name, value)| (name.clone(), value_to_json(value)))
                .collect::<serde_json::Map<String, serde_json::Value>>(),
            "context": { "channel": channel },
        });

        let reply = self.runtime.invoke_pinned_artifact(TurnSubmit {
            tenant: self.tenant_id.clone(),
            instance_id: instance_id.clone(),
            flow_id: flow_id.clone(),
            flow_rev: version.to_string(),
            input,
        });
        // Fetch the append-only ledger regardless of invocation outcome. An effect that reached
        // the kernel must remain visible in the human decision narrative even when the host/tool
        // returned a typed error.
        if let Ok(trace) =
            self.runtime
                .invocation_trace(&self.tenant_id, &instance_id, &flow_id, version)
        {
            self.last_trace = Some(
                serde_json::to_string(&trace)
                    .map_err(|error| AelioError::new(ReasonCode::Internal, error.to_string()))?,
            );
        }
        let reply = reply.map_err(map_runtime_error)?;

        match reply {
            TurnReply::Completed { bag, .. } => {
                bag.get("result").map(json_to_value).ok_or_else(|| {
                    AelioError::new(
                        ReasonCode::ToolError,
                        format!("tool {} completed without a result", tool.id),
                    )
                })
            }
            TurnReply::Parked { .. } => Err(AelioError::new(
                ReasonCode::Unavailable,
                format!("tool {} parked instead of completing", tool.id),
            )),
        }
    }

    fn take_decision_trace(&mut self) -> Option<String> {
        self.last_trace.take()
    }
}

fn parse_version(tool: &ToolSpec) -> AelioResult<u32> {
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
    Ok(version)
}

fn stable_instance_id(
    tenant_id: &str,
    tool_id: &str,
    version: u32,
    idempotency_key: &str,
) -> String {
    let mut hasher = Sha256::new();
    for part in [tenant_id, tool_id, &version.to_string(), idempotency_key] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("agent-tool-{}", hex::encode(hasher.finalize()))
}

fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(value) => (*value).into(),
        Value::Int(value) => (*value).into(),
        Value::Float(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Str(value) => value.clone().into(),
        Value::List(values) => values.iter().map(value_to_json).collect(),
        Value::Map(values) => values
            .iter()
            .map(|(name, value)| (name.clone(), value_to_json(value)))
            .collect(),
    }
}

fn json_to_value(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(value) => Value::Bool(*value),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(Value::Int)
            .or_else(|| value.as_f64().map(Value::Float))
            .unwrap_or(Value::Null),
        serde_json::Value::String(value) => Value::Str(value.clone()),
        serde_json::Value::Array(values) => Value::List(values.iter().map(json_to_value).collect()),
        serde_json::Value::Object(values) => Value::Map(
            values
                .iter()
                .map(|(name, value)| (name.clone(), json_to_value(value)))
                .collect(),
        ),
    }
}

fn map_runtime_error(error: RuntimeError) -> AelioError {
    let code = match error {
        RuntimeError::Invalid(_) => ReasonCode::Validation,
        RuntimeError::Conflict(_) => ReasonCode::Conflict,
        RuntimeError::NotFound(_) => ReasonCode::NotFound,
        RuntimeError::Overloaded => ReasonCode::RateLimited,
        RuntimeError::Kernel { ref code, .. } if code.contains("denied") => ReasonCode::Denied,
        RuntimeError::Kernel { .. } => ReasonCode::ToolError,
        RuntimeError::Store(_) | RuntimeError::Host(_) => ReasonCode::Unavailable,
        RuntimeError::Internal(_) => ReasonCode::Internal,
    };
    AelioError::new(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_conversion_is_plain_json_and_lossless() {
        let source = Value::Map(IndexMap::from([
            ("ok".into(), Value::Bool(true)),
            (
                "items".into(),
                Value::List(vec![Value::Int(7), Value::Str("x".into())]),
            ),
        ]));
        assert_eq!(json_to_value(&value_to_json(&source)), source);
    }

    #[test]
    fn instance_identity_is_stable_and_pin_sensitive() {
        let first = stable_instance_id("tenant", "orders.list", 1, "idem");
        assert_eq!(
            first,
            stable_instance_id("tenant", "orders.list", 1, "idem")
        );
        assert_ne!(
            first,
            stable_instance_id("tenant", "orders.list", 2, "idem")
        );
    }
}
