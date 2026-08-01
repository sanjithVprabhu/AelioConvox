//! Deterministic prompt construction and replayable model-provider boundaries.

use crate::types::{AelioError, AelioResult, ReasonCode, Sensitivity};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSlotSpec {
    pub name: String,
    pub required: bool,
    pub sensitivity: Sensitivity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedOutputSpec {
    pub required_fields: Vec<String>,
    pub allowed_fields: Vec<String>,
    pub field_types: IndexMap<String, ClosedOutputType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedOutputType {
    String,
    StringArray,
    PathStepArray,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSpec {
    pub id: String,
    pub version: String,
    pub instruction: String,
    /// Declaration order is rendering order and therefore part of the prompt hash.
    pub slots: Vec<PromptSlotSpec>,
    pub max_chars: usize,
    pub max_tokens: usize,
    /// Slots are truncated in this exact order. Undeclared truncation is forbidden.
    pub truncation_order: Vec<String>,
    pub output: ClosedOutputSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedPrompt {
    pub spec_id: String,
    pub spec_version: String,
    pub prompt_hash: String,
    pub output_schema: Value,
    pub text: String,
    pub redacted_text: String,
    pub estimated_tokens: usize,
    pub truncated_slots: Vec<String>,
}

impl PromptSpec {
    pub fn validate(&self) -> AelioResult<()> {
        if self.id.trim().is_empty() || self.version.trim().is_empty() {
            return Err(validation("prompt id and version are required"));
        }
        if self.max_chars == 0 || self.max_tokens == 0 {
            return Err(validation("prompt budgets must be greater than zero"));
        }
        let mut names = std::collections::HashSet::new();
        for slot in &self.slots {
            if slot.name.trim().is_empty() || !names.insert(slot.name.as_str()) {
                return Err(validation("prompt slot names must be non-empty and unique"));
            }
        }
        let mut truncation_names = std::collections::HashSet::new();
        for name in &self.truncation_order {
            if !names.contains(name.as_str()) {
                return Err(validation(format!(
                    "truncation slot `{name}` is not declared"
                )));
            }
            if !truncation_names.insert(name.as_str()) {
                return Err(validation(format!(
                    "truncation slot `{name}` is declared more than once"
                )));
            }
        }
        let allowed: std::collections::HashSet<_> = self
            .output
            .allowed_fields
            .iter()
            .map(String::as_str)
            .collect();
        if self
            .output
            .required_fields
            .iter()
            .any(|field| !allowed.contains(field.as_str()))
        {
            return Err(validation("required output fields must also be allowed"));
        }
        let required: std::collections::HashSet<_> = self
            .output
            .required_fields
            .iter()
            .map(String::as_str)
            .collect();
        if required != allowed {
            return Err(validation(
                "strict output schemas require every allowed field",
            ));
        }
        let typed: std::collections::HashSet<_> =
            self.output.field_types.keys().map(String::as_str).collect();
        if typed != allowed {
            return Err(validation(
                "every allowed output field must have exactly one declared type",
            ));
        }
        Ok(())
    }

    /// Hashes the full versioned declaration, including ordering and budgets.
    pub fn canonical_hash(&self) -> AelioResult<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| validation(format!("cannot serialize prompt spec: {error}")))?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    pub fn render(&self, bindings: &IndexMap<String, String>) -> AelioResult<RenderedPrompt> {
        self.validate()?;
        for slot in &self.slots {
            if slot.required && !bindings.contains_key(&slot.name) {
                return Err(AelioError::new(
                    ReasonCode::Missing,
                    format!("required prompt slot `{}` is missing", slot.name),
                ));
            }
        }
        if let Some(unknown) = bindings
            .keys()
            .find(|name| !self.slots.iter().any(|slot| &slot.name == *name))
        {
            return Err(validation(format!(
                "binding `{unknown}` is not a declared prompt slot"
            )));
        }

        let mut values = bindings.clone();
        let mut truncated_slots = Vec::new();
        let fits = |candidate: &IndexMap<String, String>| {
            let text = self.render_text(candidate, false);
            text.chars().count() <= self.max_chars && estimate_tokens(&text) <= self.max_tokens
        };
        if !fits(&values) {
            for name in &self.truncation_order {
                let Some(value) = values.get(name).cloned() else {
                    continue;
                };
                let chars: Vec<char> = value.chars().collect();
                let mut low = 0usize;
                let mut high = chars.len();
                while low < high {
                    let middle = (low + high).div_ceil(2);
                    values.insert(name.clone(), chars[..middle].iter().collect());
                    if fits(&values) {
                        low = middle;
                    } else {
                        high = middle - 1;
                    }
                }
                values.insert(name.clone(), chars[..low].iter().collect());
                truncated_slots.push(name.clone());
                if fits(&values) {
                    break;
                }
            }
        }
        if !fits(&values) {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                "prompt cannot fit budgets using its declared truncation order",
            ));
        }

        let text = self.render_text(&values, false);
        Ok(RenderedPrompt {
            spec_id: self.id.clone(),
            spec_version: self.version.clone(),
            prompt_hash: self.canonical_hash()?,
            output_schema: self.strict_output_schema(),
            redacted_text: self.render_text(&values, true),
            estimated_tokens: estimate_tokens(&text),
            text,
            truncated_slots,
        })
    }

    pub fn parse_output(&self, text: &str) -> AelioResult<Value> {
        self.validate()?;
        let value: Value = serde_json::from_str(text)
            .map_err(|error| AelioError::new(ReasonCode::ParseError, error.to_string()))?;
        let object = value.as_object().ok_or_else(|| {
            AelioError::new(
                ReasonCode::ParseError,
                "provider output must be a JSON object",
            )
        })?;
        for required in &self.output.required_fields {
            if !object.contains_key(required) {
                return Err(AelioError::new(
                    ReasonCode::ParseError,
                    format!("provider output is missing `{required}`"),
                ));
            }
        }
        if let Some(unknown) = object
            .keys()
            .find(|field| !self.output.allowed_fields.iter().any(|item| item == *field))
        {
            return Err(AelioError::new(
                ReasonCode::ParseError,
                format!("provider output contains undeclared field `{unknown}`"),
            ));
        }
        for (field, field_type) in &self.output.field_types {
            let Some(value) = object.get(field) else {
                continue;
            };
            let valid = match field_type {
                ClosedOutputType::String => value.is_string(),
                ClosedOutputType::StringArray => value
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string)),
                ClosedOutputType::PathStepArray => value.as_array().is_some_and(|items| {
                    items.iter().all(|item| {
                        item.as_object().is_some_and(|object| {
                            object.len() == 2
                                && object.get("ability_id").is_some_and(Value::is_string)
                                && object.get("args").is_some_and(Value::is_object)
                        })
                    })
                }),
            };
            if !valid {
                return Err(AelioError::new(
                    ReasonCode::ParseError,
                    format!("provider output field `{field}` has the wrong type"),
                ));
            }
        }
        Ok(value)
    }

    fn strict_output_schema(&self) -> Value {
        let properties = self
            .output
            .field_types
            .iter()
            .map(|(name, field_type)| {
                let schema = match field_type {
                    ClosedOutputType::String => serde_json::json!({"type": "string"}),
                    ClosedOutputType::StringArray => {
                        serde_json::json!({"type": "array", "items": {"type": "string"}})
                    }
                    ClosedOutputType::PathStepArray => serde_json::json!({
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "ability_id": {"type": "string"},
                                "args": {"type": "object"}
                            },
                            "required": ["ability_id", "args"],
                            "additionalProperties": false
                        }
                    }),
                };
                (name.clone(), schema)
            })
            .collect::<serde_json::Map<_, _>>();
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": self.output.required_fields,
            "additionalProperties": false
        })
    }

    fn render_text(&self, bindings: &IndexMap<String, String>, redact: bool) -> String {
        let mut text = format!("{}\n", self.instruction);
        for slot in &self.slots {
            if let Some(value) = bindings.get(&slot.name) {
                let value = if redact {
                    redact_value(value, slot.sensitivity)
                } else {
                    value.clone()
                };
                text.push_str(&format!("<{}>\n{}\n</{}>\n", slot.name, value, slot.name));
            }
        }
        text
    }
}

fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let rest = text.split_once(start)?.1;
    Some(rest.split_once(end)?.0)
}

fn redact_value(value: &str, sensitivity: Sensitivity) -> String {
    match sensitivity {
        Sensitivity::None => value.to_string(),
        Sensitivity::Pii => "[REDACTED:pii]".to_string(),
        Sensitivity::Secret => "[REDACTED:secret]".to_string(),
    }
}

fn validation(message: impl Into<String>) -> AelioError {
    AelioError::new(ReasonCode::Validation, message)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmRequest {
    pub prompt: RenderedPrompt,
    pub model: String,
    pub temperature: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    pub provider_request_id: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    pub texts: Vec<String>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    pub vectors: Vec<Vec<f32>>,
    pub provider_request_id: Option<String>,
    pub input_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderCall {
    Llm {
        sequence: u64,
        prompt_hash: String,
        spec_id: String,
        model: String,
        redacted_prompt: String,
        response: Option<LlmResponse>,
        error_code: Option<ReasonCode>,
        elapsed_ms: u64,
    },
    Embedding {
        sequence: u64,
        model: String,
        text_count: usize,
        text_hashes: Vec<String>,
        dimensions: Option<usize>,
        error_code: Option<ReasonCode>,
        elapsed_ms: u64,
    },
}

pub trait LlmProvider: Send {
    fn complete(&mut self, request: &LlmRequest) -> AelioResult<LlmResponse>;
    fn calls(&self) -> &[ProviderCall];

    /// True only for a configured provider that can perform language understanding. Deterministic
    /// test providers and the fail-closed placeholder return false so heuristic pre-gates do not
    /// accidentally consume scripted responses intended for a later architectural stage.
    fn supports_language_intelligence(&self) -> bool {
        true
    }
}

pub trait EmbeddingProvider: Send {
    fn embed(&mut self, request: &EmbeddingRequest) -> AelioResult<EmbeddingResponse>;
    fn calls(&self) -> &[ProviderCall];
}

/// Fail-closed provider for production runtimes that have not configured an LLM endpoint. Unlike
/// the scripted provider used by tests and examples, this can never manufacture a completion.
#[derive(Debug, Default, Clone)]
pub struct UnavailableLlmProvider {
    calls: Vec<ProviderCall>,
}

impl LlmProvider for UnavailableLlmProvider {
    fn complete(&mut self, request: &LlmRequest) -> AelioResult<LlmResponse> {
        let error = AelioError::new(ReasonCode::Unavailable, "no LLM provider is configured");
        let result = Err(error.clone());
        self.calls.push(llm_record(
            self.calls.len() as u64,
            request,
            &result,
            Duration::ZERO,
        ));
        Err(error)
    }

    fn calls(&self) -> &[ProviderCall] {
        &self.calls
    }

    fn supports_language_intelligence(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct ScriptedLlmProvider {
    responses: VecDeque<AelioResult<LlmResponse>>,
    fallback_by_spec: IndexMap<String, String>,
    calls: Vec<ProviderCall>,
}

impl Default for ScriptedLlmProvider {
    fn default() -> Self {
        Self::deterministic()
    }
}

impl ScriptedLlmProvider {
    pub fn new(responses: impl IntoIterator<Item = AelioResult<LlmResponse>>) -> Self {
        Self {
            responses: responses.into_iter().collect(),
            fallback_by_spec: IndexMap::new(),
            calls: Vec::new(),
        }
    }

    pub fn deterministic() -> Self {
        Self {
            responses: VecDeque::new(),
            fallback_by_spec: indexmap::indexmap! {
                "aelio.propose_path".into() =>
                    r#"{"steps":[{"ability_id":"State.Direction","args":{}},{"ability_id":"Registry.Capabilities","args":{}},{"ability_id":"Express.Template","args":{}}]}"#.into(),
            },
            calls: Vec::new(),
        }
    }

    pub fn with_fallback(mut self, spec_id: impl Into<String>, output: impl Into<String>) -> Self {
        self.fallback_by_spec.insert(spec_id.into(), output.into());
        self
    }
}

impl LlmProvider for ScriptedLlmProvider {
    fn complete(&mut self, request: &LlmRequest) -> AelioResult<LlmResponse> {
        let started = Instant::now();
        let result = self.responses.pop_front().unwrap_or_else(|| {
            let dynamic = (request.prompt.spec_id == "aelio.synthesize").then(|| {
                let evidence = between(&request.prompt.text, "<evidence>\n", "\n</evidence>")
                    .unwrap_or_default();
                if request.prompt.output_schema["properties"]
                    .get("claim_refs")
                    .is_some()
                {
                    let claims = serde_json::from_str::<Vec<Value>>(evidence).unwrap_or_default();
                    let claim_refs: Vec<String> = claims
                        .iter()
                        .filter_map(|claim| {
                            claim.get("id").and_then(Value::as_str).map(str::to_owned)
                        })
                        .collect();
                    let text = claims
                        .first()
                        .and_then(|claim| claim.get("value"))
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| value.to_string())
                        })
                        .unwrap_or_else(|| evidence.to_string());
                    serde_json::json!({"text": text, "claim_refs": claim_refs}).to_string()
                } else {
                    serde_json::json!({"text": evidence}).to_string()
                }
            });
            dynamic
                .or_else(|| self.fallback_by_spec.get(&request.prompt.spec_id).cloned())
                .map(|content| LlmResponse {
                    content,
                    provider_request_id: None,
                    input_tokens: Some(request.prompt.estimated_tokens as u64),
                    output_tokens: None,
                })
                .ok_or_else(|| {
                    AelioError::new(ReasonCode::Unavailable, "scripted response queue is empty")
                })
        });
        self.calls.push(llm_record(
            self.calls.len() as u64,
            request,
            &result,
            started.elapsed(),
        ));
        result
    }

    fn calls(&self) -> &[ProviderCall] {
        &self.calls
    }

    fn supports_language_intelligence(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub struct ScriptedEmbeddingProvider {
    responses: VecDeque<AelioResult<EmbeddingResponse>>,
    calls: Vec<ProviderCall>,
}

impl ScriptedEmbeddingProvider {
    pub fn new(responses: impl IntoIterator<Item = AelioResult<EmbeddingResponse>>) -> Self {
        Self {
            responses: responses.into_iter().collect(),
            calls: Vec::new(),
        }
    }
}

impl EmbeddingProvider for ScriptedEmbeddingProvider {
    fn embed(&mut self, request: &EmbeddingRequest) -> AelioResult<EmbeddingResponse> {
        let started = Instant::now();
        let result = self.responses.pop_front().unwrap_or_else(|| {
            Err(AelioError::new(
                ReasonCode::Unavailable,
                "scripted embedding response queue is empty",
            ))
        });
        self.calls.push(embedding_record(
            self.calls.len() as u64,
            request,
            &result,
            started.elapsed(),
        ));
        result
    }

    fn calls(&self) -> &[ProviderCall] {
        &self.calls
    }
}

#[derive(Debug, Clone)]
pub struct HttpProviderConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    pub extra_headers: IndexMap<String, String>,
}

pub struct OpenAiCompatibleProvider {
    config: HttpProviderConfig,
    client: reqwest::blocking::Client,
    calls: Vec<ProviderCall>,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: HttpProviderConfig) -> AelioResult<Self> {
        if config.endpoint.trim().is_empty() || config.model.trim().is_empty() {
            return Err(validation("provider endpoint and model are required"));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| AelioError::new(ReasonCode::Unavailable, error.to_string()))?;
        Ok(Self {
            config,
            client,
            calls: Vec::new(),
        })
    }

    /// Build from `OPENAI_API_KEY` (required) and optional `OPENAI_MODEL` / `OPENAI_BASE_URL`.
    pub fn from_env() -> AelioResult<Self> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            AelioError::new(
                ReasonCode::Unavailable,
                "OPENAI_API_KEY is not set — cannot run live LLM tests",
            )
        })?;
        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
        let endpoint = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1/chat/completions".into());
        Self::new(HttpProviderConfig {
            endpoint,
            model,
            api_key: Some(api_key),
            timeout: Duration::from_secs(45),
            extra_headers: IndexMap::new(),
        })
    }

    pub fn call_count(&self) -> usize {
        self.calls.len()
    }
}

impl LlmProvider for OpenAiCompatibleProvider {
    fn complete(&mut self, request: &LlmRequest) -> AelioResult<LlmResponse> {
        let started = Instant::now();
        let schema_name = request
            .prompt
            .spec_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let mut builder = self
            .client
            .post(&self.config.endpoint)
            .json(&serde_json::json!({
                "model": if request.model == "default" {
                    &self.config.model
                } else {
                    &request.model
                },
                "temperature": request.temperature,
                "messages": [{"role": "user", "content": request.prompt.text}],
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {
                        "name": schema_name,
                        "strict": true,
                        "schema": request.prompt.output_schema
                    }
                }
            }));
        if let Some(api_key) = &self.config.api_key {
            builder = builder.bearer_auth(api_key);
        }
        for (name, value) in &self.config.extra_headers {
            builder = builder.header(name, value);
        }
        let result = builder
            .send()
            .map_err(map_reqwest_error)
            .and_then(|response| {
                let status = response.status();
                if !status.is_success() {
                    return Err(AelioError::new(
                        if status.as_u16() == 429 {
                            ReasonCode::RateLimited
                        } else {
                            ReasonCode::Unavailable
                        },
                        format!("provider returned HTTP {status}"),
                    ));
                }
                let request_id = response
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                let body: Value = response
                    .json()
                    .map_err(|error| AelioError::new(ReasonCode::ParseError, error.to_string()))?;
                let content = body
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::ParseError,
                            "provider response has no message content",
                        )
                    })?
                    .to_string();
                Ok(LlmResponse {
                    content,
                    provider_request_id: request_id,
                    input_tokens: body.pointer("/usage/prompt_tokens").and_then(Value::as_u64),
                    output_tokens: body
                        .pointer("/usage/completion_tokens")
                        .and_then(Value::as_u64),
                })
            });
        self.calls.push(llm_record(
            self.calls.len() as u64,
            request,
            &result,
            started.elapsed(),
        ));
        result
    }

    fn calls(&self) -> &[ProviderCall] {
        &self.calls
    }
}

/// Production provider: dials the TypeScript multi-LLM gateway over HTTP.
///
/// The gateway is a stateless "modem" — it picks a vendor SDK, handles vendor auth and quirks, and
/// returns raw text. It owns **no** decisioning: no tool calling, no flow/tier selection, no policy.
/// Rust builds the closed-schema `PromptSpec`, dials this, and validates the reply against the same
/// schema. That keeps the determinism sandwich intact regardless of which vendor answered, and keeps
/// the blast radius small — a gateway outage never touches durable turn / CAS logic.
pub struct TsGatewayProvider {
    config: HttpProviderConfig,
    client: reqwest::blocking::Client,
    calls: Vec<ProviderCall>,
}

impl TsGatewayProvider {
    pub fn new(config: HttpProviderConfig) -> AelioResult<Self> {
        if config.endpoint.trim().is_empty() {
            return Err(validation("gateway endpoint is required"));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| AelioError::new(ReasonCode::Unavailable, error.to_string()))?;
        Ok(Self {
            config,
            client,
            calls: Vec::new(),
        })
    }

    /// Build from `AELIO_LLM_GATEWAY_URL` (required), optional `AELIO_LLM_GATEWAY_TOKEN` (auth to the
    /// gateway itself — not a vendor key) and `AELIO_LLM_MODEL` (default routing model/alias).
    pub fn from_env() -> AelioResult<Self> {
        let endpoint = std::env::var("AELIO_LLM_GATEWAY_URL").map_err(|_| {
            AelioError::new(
                ReasonCode::Unavailable,
                "AELIO_LLM_GATEWAY_URL is not set — cannot reach the LLM gateway",
            )
        })?;
        let model = std::env::var("AELIO_LLM_MODEL").unwrap_or_else(|_| "tier:cheap".into());
        Self::new(HttpProviderConfig {
            endpoint,
            model,
            api_key: std::env::var("AELIO_LLM_GATEWAY_TOKEN").ok(),
            timeout: Duration::from_secs(45),
            extra_headers: IndexMap::new(),
        })
    }

    pub fn call_count(&self) -> usize {
        self.calls.len()
    }
}

/// Stable per-request id so a retried identical completion dedups gateway-side. Deterministic in the
/// request only (prompt hash + model + temperature), never wall-clock, so replay stays byte-stable.
fn gateway_request_id(request: &LlmRequest, model: &str) -> String {
    let seed = format!(
        "{}|{}|{}|{:.4}",
        request.prompt.prompt_hash, request.prompt.spec_id, model, request.temperature
    );
    format!(
        "aelio-{}",
        hex::encode(&Sha256::digest(seed.as_bytes())[..12])
    )
}

/// Pure body builder — factored out so the wire contract is unit-testable without a live gateway.
fn gateway_request_body(request: &LlmRequest, model: &str, request_id: &str) -> Value {
    serde_json::json!({
        "request_id": request_id,
        "model": model,
        "temperature": request.temperature,
        "prompt": {
            "spec_id": request.prompt.spec_id,
            "spec_version": request.prompt.spec_version,
            "prompt_hash": request.prompt.prompt_hash,
            "text": request.prompt.text,
        },
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": schema_name_for(&request.prompt.spec_id),
                "strict": true,
                "schema": request.prompt.output_schema,
            }
        }
    })
}

/// Pure response parser — the gateway returns a unified `{content, usage, provider, model}` shape.
fn parse_gateway_response(body: &Value) -> AelioResult<LlmResponse> {
    let content = body
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AelioError::new(
                ReasonCode::ParseError,
                "gateway response is missing string `content`",
            )
        })?
        .to_string();
    Ok(LlmResponse {
        content,
        provider_request_id: body
            .get("provider_request_id")
            .or_else(|| body.get("request_id"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        input_tokens: body.pointer("/usage/input_tokens").and_then(Value::as_u64),
        output_tokens: body.pointer("/usage/output_tokens").and_then(Value::as_u64),
    })
}

fn schema_name_for(spec_id: &str) -> String {
    spec_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

impl LlmProvider for TsGatewayProvider {
    fn complete(&mut self, request: &LlmRequest) -> AelioResult<LlmResponse> {
        let started = Instant::now();
        let model = if request.model == "default" {
            self.config.model.clone()
        } else {
            request.model.clone()
        };
        let request_id = gateway_request_id(request, &model);
        let body = gateway_request_body(request, &model, &request_id);
        let mut builder = self.client.post(&self.config.endpoint).json(&body);
        // Auth to the gateway itself (a shared service token), never a vendor key from Rust.
        if let Some(token) = &self.config.api_key {
            builder = builder.bearer_auth(token);
        }
        builder = builder.header("idempotency-key", &request_id);
        for (name, value) in &self.config.extra_headers {
            builder = builder.header(name, value);
        }
        let result = builder
            .send()
            .map_err(map_reqwest_error)
            .and_then(|response| {
                let status = response.status();
                if !status.is_success() {
                    return Err(AelioError::new(
                        if status.as_u16() == 429 {
                            ReasonCode::RateLimited
                        } else {
                            ReasonCode::Unavailable
                        },
                        format!("gateway returned HTTP {status}"),
                    ));
                }
                let payload: Value = response
                    .json()
                    .map_err(|error| AelioError::new(ReasonCode::ParseError, error.to_string()))?;
                parse_gateway_response(&payload)
            });
        self.calls.push(llm_record(
            self.calls.len() as u64,
            request,
            &result,
            started.elapsed(),
        ));
        result
    }

    fn calls(&self) -> &[ProviderCall] {
        &self.calls
    }
}

fn llm_record(
    sequence: u64,
    request: &LlmRequest,
    result: &AelioResult<LlmResponse>,
    elapsed: Duration,
) -> ProviderCall {
    ProviderCall::Llm {
        sequence,
        prompt_hash: request.prompt.prompt_hash.clone(),
        spec_id: request.prompt.spec_id.clone(),
        model: request.model.clone(),
        redacted_prompt: request.prompt.redacted_text.clone(),
        response: result.as_ref().ok().cloned(),
        error_code: result.as_ref().err().map(|error| error.code),
        elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
    }
}

fn embedding_record(
    sequence: u64,
    request: &EmbeddingRequest,
    result: &AelioResult<EmbeddingResponse>,
    elapsed: Duration,
) -> ProviderCall {
    ProviderCall::Embedding {
        sequence,
        model: request.model.clone(),
        text_count: request.texts.len(),
        text_hashes: request
            .texts
            .iter()
            .map(|text| hex::encode(Sha256::digest(text.as_bytes())))
            .collect(),
        dimensions: result
            .as_ref()
            .ok()
            .and_then(|response| response.vectors.first().map(Vec::len)),
        error_code: result.as_ref().err().map(|error| error.code),
        elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
    }
}

fn map_reqwest_error(error: reqwest::Error) -> AelioError {
    let code = if error.is_timeout() {
        ReasonCode::Timeout
    } else {
        ReasonCode::Unavailable
    };
    AelioError::new(code, error.to_string())
}

/// A real embedding model behind the `Embedder` trait, served by the TS gateway's `/embed` route.
///
/// This is the keystone the architecture was missing: with a genuine semantic space, σ near-match
/// (Tier-1), term bridging ("avaricious"→declared "greedy"), and intent classification stop falling
/// back to bag-of-hash and start generalizing. Hot paths re-embed the same anchors/labels every
/// turn, so vectors are cached by text — a stable anchor is embedded once, then free.
pub struct GatewayEmbedder {
    endpoint: String,
    model: String,
    api_key: Option<String>,
    dimension: usize,
    client: reqwest::blocking::Client,
    /// Keys are one-way text fingerprints, never raw user text. The cache has a hard structural
    /// bound so a long-running process cannot grow without limit.
    cache: std::sync::Mutex<std::collections::HashMap<String, Vec<f32>>>,
}

impl GatewayEmbedder {
    const MAX_INPUT_CHARS: usize = 32_768;
    const MAX_CACHE_ENTRIES: usize = 4_096;

    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
        dimension: usize,
        timeout: Duration,
    ) -> AelioResult<Self> {
        let endpoint = endpoint.into();
        if endpoint.trim().is_empty() {
            return Err(validation("embedding endpoint is required"));
        }
        let model = model.into();
        if model.trim().is_empty() {
            return Err(validation("embedding model is required"));
        }
        if dimension == 0 {
            return Err(validation("embedding dimension must be positive"));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| AelioError::new(ReasonCode::Unavailable, error.to_string()))?;
        Ok(Self {
            endpoint,
            model,
            api_key,
            dimension,
            client,
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Build from `AELIO_LLM_EMBED_URL` (or derive it from the canonical
    /// `AELIO_LLM_GATEWAY_URL` completion route), `AELIO_LLM_EMBED_MODEL`, and
    /// `AELIO_LLM_EMBED_DIM`.
    ///
    /// A non-canonical gateway completion URL cannot be derived safely: silently sending an
    /// embedding request to a completion route is worse than a clear startup error. Operators
    /// using a custom route must set `AELIO_LLM_EMBED_URL` explicitly.
    pub fn from_env() -> AelioResult<Self> {
        let endpoint = std::env::var("AELIO_LLM_EMBED_URL")
            .ok()
            .or_else(|| {
                std::env::var("AELIO_LLM_GATEWAY_URL")
                    .ok()
                    .and_then(|url| derive_embed_endpoint(&url))
            })
            .ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Unavailable,
                    "AELIO_LLM_EMBED_URL / AELIO_LLM_GATEWAY_URL is not set",
                )
            })?;
        let model = std::env::var("AELIO_LLM_EMBED_MODEL").unwrap_or_else(|_| "tier:embed".into());
        let dimension = match std::env::var("AELIO_LLM_EMBED_DIM") {
            Ok(value) => value
                .parse()
                .map_err(|_| validation("AELIO_LLM_EMBED_DIM must be a positive integer"))?,
            Err(std::env::VarError::NotPresent) => 1536,
            Err(error) => {
                return Err(validation(format!(
                    "cannot read AELIO_LLM_EMBED_DIM: {error}"
                )));
            }
        };
        Self::new(
            endpoint,
            model,
            std::env::var("AELIO_LLM_GATEWAY_TOKEN").ok(),
            dimension,
            Duration::from_secs(30),
        )
    }

    fn fetch(&self, text: &str) -> AelioResult<Vec<f32>> {
        if text.chars().count() > Self::MAX_INPUT_CHARS {
            return Err(AelioError::new(
                ReasonCode::BudgetExceeded,
                format!(
                    "embedding input exceeds {} characters",
                    Self::MAX_INPUT_CHARS
                ),
            ));
        }
        let request_id = format!(
            "aelio-embed-{}",
            hex::encode(&Sha256::digest(format!("{}|{}", self.model, text).as_bytes())[..12])
        );
        let body = embed_request_body(
            &self.model,
            std::slice::from_ref(&text.to_string()),
            &request_id,
        );
        let mut builder = self.client.post(&self.endpoint).json(&body);
        if let Some(token) = &self.api_key {
            builder = builder.bearer_auth(token);
        }
        let payload: Value = builder
            .send()
            .map_err(map_reqwest_error)
            .and_then(|response| {
                if !response.status().is_success() {
                    return Err(AelioError::new(
                        ReasonCode::Unavailable,
                        format!("embed gateway returned HTTP {}", response.status()),
                    ));
                }
                response
                    .json()
                    .map_err(|error| AelioError::new(ReasonCode::ParseError, error.to_string()))
            })?;
        let vector = parse_embed_vectors(&payload)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                AelioError::new(ReasonCode::ParseError, "embed response had no vectors")
            })?;
        if vector.len() != self.dimension {
            return Err(AelioError::new(
                ReasonCode::Validation,
                format!(
                    "embedding dimension mismatch: expected {}, got {}",
                    self.dimension,
                    vector.len()
                ),
            ));
        }
        Ok(vector)
    }
}

impl crate::embedding::Embedder for GatewayEmbedder {
    fn dimension(&self) -> usize {
        self.dimension
    }

    fn embed(&self, text: &str) -> AelioResult<Vec<f32>> {
        let cache_key = hex::encode(Sha256::digest(
            format!("{}|{}", self.model, text).as_bytes(),
        ));
        if let Ok(cache) = self.cache.lock() {
            if let Some(vector) = cache.get(&cache_key) {
                return Ok(vector.clone());
            }
        }
        let vector = self.fetch(text)?;
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() < Self::MAX_CACHE_ENTRIES {
                cache.insert(cache_key, vector.clone());
            }
        }
        Ok(vector)
    }

    fn supports_semantic_equivalence(&self) -> bool {
        true
    }

    fn space_id(&self) -> String {
        let identity = format!("{}|{}|{}", self.endpoint, self.model, self.dimension);
        format!(
            "aelio.gateway:{}",
            hex::encode(&Sha256::digest(identity.as_bytes())[..16])
        )
    }
}

fn derive_embed_endpoint(gateway_url: &str) -> Option<String> {
    let (base, query) = gateway_url
        .split_once('?')
        .map_or((gateway_url, None), |(base, query)| (base, Some(query)));
    let base = base.strip_suffix("/v1/llm/complete")?;
    let mut endpoint = format!("{base}/v1/llm/embed");
    if let Some(query) = query {
        endpoint.push('?');
        endpoint.push_str(query);
    }
    Some(endpoint)
}

/// Pure request body for the gateway `/embed` route — factored out so the wire contract is testable.
fn embed_request_body(model: &str, texts: &[String], request_id: &str) -> Value {
    serde_json::json!({
        "request_id": request_id,
        "model": model,
        "input": texts,
    })
}

/// Pure parser for the gateway `/embed` response `{ vectors: [[f32]], model, provider }`.
fn parse_embed_vectors(body: &Value) -> AelioResult<Vec<Vec<f32>>> {
    let vectors = body
        .get("vectors")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AelioError::new(
                ReasonCode::ParseError,
                "embed response is missing array `vectors`",
            )
        })?;
    vectors
        .iter()
        .map(|vector| {
            vector
                .as_array()
                .ok_or_else(|| {
                    AelioError::new(ReasonCode::ParseError, "embed vector must be an array")
                })?
                .iter()
                .map(|value| {
                    value.as_f64().map(|f| f as f32).ok_or_else(|| {
                        AelioError::new(ReasonCode::ParseError, "embed value must be a number")
                    })
                })
                .collect::<AelioResult<Vec<f32>>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> PromptSpec {
        PromptSpec {
            id: "test".into(),
            version: "2".into(),
            instruction: "Return JSON.".into(),
            slots: vec![
                PromptSlotSpec {
                    name: "public".into(),
                    required: true,
                    sensitivity: Sensitivity::None,
                },
                PromptSlotSpec {
                    name: "secret".into(),
                    required: true,
                    sensitivity: Sensitivity::Secret,
                },
            ],
            max_chars: 120,
            max_tokens: 30,
            truncation_order: vec!["public".into()],
            output: ClosedOutputSpec {
                required_fields: vec!["answer".into()],
                allowed_fields: vec!["answer".into()],
                field_types: indexmap::indexmap! {
                    "answer".into() => ClosedOutputType::String,
                },
            },
        }
    }

    #[test]
    fn hash_and_rendering_are_deterministic_and_redacted() {
        let spec = spec();
        let bindings = indexmap::indexmap! {
            "secret".into() => "swordfish".into(),
            "public".into() => "hello".into(),
        };
        let rendered = spec.render(&bindings).unwrap();
        assert!(rendered.text.find("<public>").unwrap() < rendered.text.find("<secret>").unwrap());
        assert!(rendered.text.contains("swordfish"));
        assert!(!rendered.redacted_text.contains("swordfish"));
        assert_eq!(rendered.prompt_hash, spec.canonical_hash().unwrap());
    }

    #[test]
    fn validates_required_slots_and_closed_output() {
        assert_eq!(
            spec().render(&IndexMap::new()).unwrap_err().code,
            ReasonCode::Missing
        );
        assert!(spec().parse_output(r#"{"answer":"ok"}"#).is_ok());
        assert_eq!(
            spec()
                .parse_output(r#"{"answer":"ok","extra":true}"#)
                .unwrap_err()
                .code,
            ReasonCode::ParseError
        );
        assert_eq!(
            spec().parse_output(r#"{"answer":42}"#).unwrap_err().code,
            ReasonCode::ParseError
        );
        assert_eq!(
            spec()
                .render(&indexmap::indexmap! {
                    "public".into() => "hello".into(),
                    "secret".into() => "swordfish".into(),
                })
                .unwrap()
                .output_schema,
            serde_json::json!({
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"],
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn rejects_output_with_omitted_required_field() {
        let error = spec().parse_output("{}").unwrap_err();
        assert_eq!(error.code, ReasonCode::ParseError);
        assert!(error.message.contains("answer"));
    }

    #[test]
    fn truncates_only_in_declared_order() {
        let rendered = spec()
            .render(&indexmap::indexmap! {
                "public".into() => "x".repeat(200),
                "secret".into() => "short".into(),
            })
            .unwrap();
        assert_eq!(rendered.truncated_slots, vec!["public"]);
        assert!(rendered.text.chars().count() <= spec().max_chars);
    }

    #[test]
    fn scripted_calls_are_replay_assertable() {
        let prompt = spec()
            .render(&indexmap::indexmap! {
                "public".into() => "hello".into(),
                "secret".into() => "swordfish".into(),
            })
            .unwrap();
        let mut provider = ScriptedLlmProvider::new([Ok(LlmResponse {
            content: r#"{"answer":"ok"}"#.into(),
            provider_request_id: Some("req-1".into()),
            input_tokens: Some(4),
            output_tokens: Some(2),
        })]);
        provider
            .complete(&LlmRequest {
                prompt,
                model: "scripted".into(),
                temperature: 0.0,
            })
            .unwrap();
        assert_eq!(provider.calls().len(), 1);
        let encoded = serde_json::to_string(provider.calls()).unwrap();
        assert!(!encoded.contains("swordfish"));
        assert!(encoded.contains("req-1"));
    }

    fn gateway_request() -> LlmRequest {
        let prompt = spec()
            .render(&indexmap::indexmap! {
                "public".into() => "hello".into(),
                "secret".into() => "swordfish".into(),
            })
            .unwrap();
        LlmRequest {
            prompt,
            model: "tier:cheap".into(),
            temperature: 0.0,
        }
    }

    #[test]
    fn gateway_body_carries_closed_schema_and_no_free_form_escape() {
        let request = gateway_request();
        let id = gateway_request_id(&request, "tier:cheap");
        let body = gateway_request_body(&request, "tier:cheap", &id);

        assert_eq!(body["model"], "tier:cheap");
        assert_eq!(body["request_id"], id);
        // The gateway receives the exact closed output schema Rust will re-validate against — the
        // model has no free-form escape.
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(
            body["response_format"]["json_schema"]["schema"],
            request.prompt.output_schema
        );
        assert_eq!(body["prompt"]["prompt_hash"], request.prompt.prompt_hash);
    }

    #[test]
    fn gateway_request_id_is_stable_and_replay_safe() {
        let request = gateway_request();
        // Deterministic in the request only — a retry of an identical completion dedups gateway-side.
        assert_eq!(
            gateway_request_id(&request, "tier:cheap"),
            gateway_request_id(&request, "tier:cheap")
        );
        // A different routed model is a different logical call.
        assert_ne!(
            gateway_request_id(&request, "tier:cheap"),
            gateway_request_id(&request, "claude-sonnet-5")
        );
    }

    #[test]
    fn parse_gateway_response_maps_unified_shape() {
        let body = serde_json::json!({
            "content": r#"{"answer":"ok"}"#,
            "provider": "openai",
            "model": "gpt-4o-mini",
            "provider_request_id": "openai-req-9",
            "usage": { "input_tokens": 11, "output_tokens": 3 }
        });
        let parsed = parse_gateway_response(&body).unwrap();
        assert_eq!(parsed.content, r#"{"answer":"ok"}"#);
        assert_eq!(parsed.provider_request_id.as_deref(), Some("openai-req-9"));
        assert_eq!(parsed.input_tokens, Some(11));
        assert_eq!(parsed.output_tokens, Some(3));
        // And the closed schema still gets the final say on the model's text.
        assert!(spec().parse_output(&parsed.content).is_ok());
    }

    #[test]
    fn parse_gateway_response_rejects_missing_content() {
        let body = serde_json::json!({ "usage": { "input_tokens": 1 } });
        assert_eq!(
            parse_gateway_response(&body).unwrap_err().code,
            ReasonCode::ParseError
        );
    }

    #[test]
    fn embed_request_body_carries_input_and_model() {
        let body = embed_request_body(
            "tier:embed",
            &["greedy".to_string(), "generous".to_string()],
            "aelio-embed-x",
        );
        assert_eq!(body["model"], "tier:embed");
        assert_eq!(body["request_id"], "aelio-embed-x");
        assert_eq!(body["input"], serde_json::json!(["greedy", "generous"]));
    }

    #[test]
    fn embed_endpoint_derivation_is_exact_and_fails_closed() {
        assert_eq!(
            derive_embed_endpoint("http://localhost:8787/v1/llm/complete").as_deref(),
            Some("http://localhost:8787/v1/llm/embed")
        );
        assert_eq!(
            derive_embed_endpoint("https://gateway.example/v1/llm/complete?region=in").as_deref(),
            Some("https://gateway.example/v1/llm/embed?region=in")
        );
        assert_eq!(
            derive_embed_endpoint("https://gateway.example/custom/complete"),
            None
        );
        assert_eq!(
            derive_embed_endpoint("https://gateway.example/v1/llm/complete/extra"),
            None
        );
    }

    #[test]
    fn parse_embed_vectors_reads_float_matrix() {
        let body = serde_json::json!({
            "vectors": [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]],
            "model": "text-embedding-3-small",
            "provider": "openai"
        });
        let vectors = parse_embed_vectors(&body).unwrap();
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0].len(), 3);
        assert!((vectors[1][2] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn parse_embed_vectors_rejects_missing_field() {
        let body = serde_json::json!({ "provider": "openai" });
        assert_eq!(
            parse_embed_vectors(&body).unwrap_err().code,
            ReasonCode::ParseError
        );
    }
}
