use aelio_agent::abilities::invoke::{CapabilityHost, ToolHost};
use aelio_agent::tenant::ToolSpec;
use aelio_agent::{AelioError, AelioResult, ReasonCode, Value};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Blocking adapter used only from the agent's dedicated blocking turn worker.
pub struct HttpAgentToolHost {
    client: reqwest::blocking::Client,
    endpoint: String,
    token: String,
    tenant: String,
}

impl HttpAgentToolHost {
    pub fn new(
        base_url: &str,
        token: String,
        tenant: String,
        timeout: Duration,
    ) -> AelioResult<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| AelioError::new(ReasonCode::Unavailable, error.to_string()))?;
        Ok(Self {
            client,
            endpoint: format!("{}/internal/aelio/target", base_url.trim_end_matches('/')),
            token,
            tenant,
        })
    }
}

#[derive(Serialize)]
struct Request<'a> {
    protocol: &'static str,
    corr: &'a str,
    tenant: &'a str,
    instance_id: &'a str,
    turn_id: &'a str,
    nid: &'static str,
    deadline_ms: Option<u64>,
    target: String,
    args: serde_json::Value,
}

#[derive(Deserialize)]
struct Response {
    outcome: String,
    output: Option<Value>,
    error: Option<ResponseError>,
}

#[derive(Deserialize)]
struct ResponseError {
    code: String,
    detail: String,
}

impl CapabilityHost for HttpAgentToolHost {
    fn call_with_context(
        &mut self,
        tool: &ToolSpec,
        args: &IndexMap<String, Value>,
        idempotency_key: &str,
        user_id: &str,
        channel: &str,
    ) -> AelioResult<Value> {
        self.invoke(
            &tool.id,
            &tool.version,
            args,
            idempotency_key,
            user_id,
            channel,
        )
    }
}

impl ToolHost for HttpAgentToolHost {
    fn call(&mut self, tool_id: &str, args: &IndexMap<String, Value>) -> AelioResult<Value> {
        self.invoke(tool_id, "1", args, tool_id, "anonymous", "unknown")
    }
}

impl HttpAgentToolHost {
    fn invoke(
        &self,
        tool_id: &str,
        version: &str,
        args: &IndexMap<String, Value>,
        correlation: &str,
        user_id: &str,
        channel: &str,
    ) -> AelioResult<Value> {
        let request = Request {
            protocol: "aelio-host/1",
            corr: correlation,
            tenant: &self.tenant,
            instance_id: user_id,
            turn_id: correlation,
            nid: "agent.invoke",
            deadline_ms: Some(30_000),
            target: format!("{tool_id}@{version}"),
            args: serde_json::json!({
                "args": args,
                "context": {"channel": channel}
            }),
        };
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.token)
            .json(&request)
            .send()
            .map_err(|error| AelioError::new(ReasonCode::Unavailable, error.to_string()))?;
        if !response.status().is_success() {
            return Err(AelioError::new(
                ReasonCode::Unavailable,
                format!("host returned HTTP {}", response.status()),
            ));
        }
        let body: Response = response
            .json()
            .map_err(|error| AelioError::new(ReasonCode::ParseError, error.to_string()))?;
        if body.outcome == "ok" {
            body.output.ok_or_else(|| {
                AelioError::new(ReasonCode::ParseError, "host response omitted output")
            })
        } else {
            let error = body.error.unwrap_or(ResponseError {
                code: "tool_transient".into(),
                detail: "host adapter failed".into(),
            });
            Err(AelioError::new(
                if error.code == "shape" {
                    ReasonCode::Validation
                } else {
                    ReasonCode::Unavailable
                },
                error.detail,
            ))
        }
    }
}
