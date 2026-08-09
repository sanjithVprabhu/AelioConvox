//! A provider-neutral agent loop with explicit completion and kernel-owned dispatch.
//!
//! The crate deliberately contains no HTTP, WebSocket, database, or provider SDK code.
//! Production integrations implement the narrow [`Model`], [`ToolHost`], and [`EffectGate`]
//! traits; deterministic tests use in-memory fakes.

mod context;
mod engine;
mod manifest;
mod progress;
mod security;
mod types;

pub use context::{Context, ContextError, ContextRewrite, RewriteReason};
pub use engine::{AgentLoop, EffectGate, Model, ToolHost};
pub use manifest::{canonical_hash, LoopManifest, ManifestError, MANIFEST_ORDERING_VERSION};
pub use progress::{ProgressMonitor, ProgressNotice, StallKind, TurnFingerprint};
pub use security::{
    compose_policy, ActionBinding, ConfirmationRecord, ConfirmationStatus, PolicyVerdict,
};
pub use types::{
    AgentMessage, AgentModelRequestV2, AgentModelResponseV2, AssistantBlock, Budget, BudgetLimits,
    CacheHintsV2, EffectClass, GateDecision, LoopError, LoopEvent, LoopOutcome, ModelRequest,
    ModelResponse, ModelUsage, StopReason, ToolCall, ToolChoiceKindV2, ToolChoiceV2,
    ToolDefinition, ToolError, ToolErrorClass, ToolResult,
};
