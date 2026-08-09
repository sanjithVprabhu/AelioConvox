use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    Read,
    WriteReversible,
    WriteIrreversible,
    Financial,
    AccessControl,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub version: String,
    pub description: String,
    pub input_schema: Value,
    pub effect_class: EffectClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantBlock {
    Text { text: String },
    ToolCall { call: ToolCall },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    ToolUse,
    EndTurn,
    MaxOutputTokens,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub id: String,
    pub content: Vec<AssistantBlock>,
    pub stop_reason: StopReason,
    pub usage: ModelUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum AgentMessage {
    User { text: String },
    Assistant { response: ModelResponse },
    ToolResult { result: ToolResult },
    KernelNotice { code: String, detail: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub system: String,
    pub bootstrap: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentModelRequestV2 {
    pub protocol_version: u8,
    pub request_id: String,
    pub attempt_id: String,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub system: String,
    pub bootstrap: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: ToolChoiceV2,
    pub cache: CacheHintsV2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolChoiceV2 {
    #[serde(rename = "type")]
    pub kind: ToolChoiceKindV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoiceKindV2 {
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheHintsV2 {
    pub stable_prefix_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentModelResponseV2 {
    pub protocol_version: u8,
    pub request_id: String,
    pub attempt_id: String,
    pub response: ModelResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorClass {
    UnknownTool,
    InvalidArguments,
    NotAuthorized,
    ConfirmationRequired,
    Timeout,
    Handler,
    ResultTooLarge,
    MixedFinishAndActions,
    CompletionRejected,
    RateLimited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolError {
    pub class: ToolErrorClass,
    pub retryable: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub tool: String,
    pub data: Option<Value>,
    pub error: Option<ToolError>,
}

impl ToolResult {
    pub fn ok(call: &ToolCall, data: Value) -> Self {
        Self {
            call_id: call.id.clone(),
            tool: call.name.clone(),
            data: Some(data),
            error: None,
        }
    }

    /// Convert a host outcome into a context-safe result without allowing an unbounded payload
    /// to enter the model transcript. This is shared by the normal loop and approval resume path.
    pub fn bounded(
        call: &ToolCall,
        outcome: Result<Value, ToolError>,
        max_result_bytes: usize,
    ) -> Self {
        match outcome {
            Ok(value) => match serde_json::to_vec(&value) {
                Ok(bytes) if bytes.len() <= max_result_bytes => Self::ok(call, value),
                Ok(_) => Self::error(
                    call,
                    ToolError {
                        class: ToolErrorClass::ResultTooLarge,
                        retryable: true,
                        detail: "tool result exceeded the configured byte limit".to_string(),
                    },
                ),
                Err(error) => Self::error(
                    call,
                    ToolError {
                        class: ToolErrorClass::Handler,
                        retryable: false,
                        detail: format!("tool result could not be serialized: {error}"),
                    },
                ),
            },
            Err(error) => Self::error(call, error),
        }
    }

    pub fn error(call: &ToolCall, error: ToolError) -> Self {
        Self {
            call_id: call.id.clone(),
            tool: call.name.clone(),
            data: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    Allow,
    Deny { reason: String },
    Confirm { reason: String },
}

#[derive(Debug, Clone)]
pub struct BudgetLimits {
    pub max_turns: u32,
    pub max_tool_calls: u32,
    pub max_protocol_violations: u32,
    pub max_total_tokens: u64,
    pub reserve_tokens: u64,
    pub wall_clock: Duration,
    pub tool_timeout: Duration,
    pub max_parallel_reads: usize,
    pub max_result_bytes: usize,
    /// Maximum retained transcript messages after a provider context-overflow signal.
    pub overflow_compaction_messages: usize,
}

impl Default for BudgetLimits {
    fn default() -> Self {
        Self {
            max_turns: 40,
            max_tool_calls: 100,
            max_protocol_violations: 3,
            max_total_tokens: 200_000,
            reserve_tokens: 4_000,
            wall_clock: Duration::from_secs(600),
            tool_timeout: Duration::from_secs(30),
            max_parallel_reads: 8,
            max_result_bytes: 128 * 1024,
            overflow_compaction_messages: 128,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Budget {
    pub turns: u32,
    pub tool_calls: u32,
    pub protocol_violations: u32,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

impl Budget {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cached_input_tokens)
            .saturating_add(self.output_tokens)
    }

    pub fn charge(&mut self, usage: ModelUsage) {
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(usage.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.turns = self.turns.saturating_add(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopOutcome {
    Finished {
        message: String,
        status: String,
    },
    WaitingForUser {
        message: String,
        pending_calls: Vec<ToolCall>,
    },
    WaitingForInput {
        message: String,
        pending_calls: Vec<ToolCall>,
        missing_fields: Vec<String>,
    },
    Cancelled {
        message: String,
    },
    Exhausted {
        reason: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopEvent {
    ModelCompleted {
        response_id: String,
    },
    ToolDenied {
        call_id: String,
        reason: String,
    },
    ToolDispatched {
        call_id: String,
        name: String,
    },
    ToolCompleted {
        call_id: String,
        ok: bool,
    },
    ProtocolViolation {
        count: u32,
        code: String,
    },
    ProgressNotice {
        kind: String,
    },
    ContextRewritten {
        reason: String,
        before_hash: String,
        after_hash: String,
    },
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error("model error: {0}")]
    Model(String),
    #[error("model context overflow: {0}")]
    ContextOverflow(String),
    #[error("context error: {0}")]
    Context(String),
    #[error("invalid configuration: {0}")]
    Configuration(String),
}
