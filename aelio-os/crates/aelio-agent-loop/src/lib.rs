//! A provider-neutral agent loop with explicit completion and kernel-owned dispatch.
//!
//! The crate deliberately contains no HTTP, WebSocket, database, or provider SDK code.
//! Production integrations implement the narrow [`Model`], [`ToolHost`], and [`EffectGate`]
//! traits; deterministic tests use in-memory fakes.

mod child;
mod compute;
mod context;
mod engine;
mod kernel_tools;
mod manifest;
mod orchestration;
mod program;
mod progress;
mod reflect;
mod security;
mod tasks;
mod todos;
mod types;

pub use child::{
    ChildExecutionRequest, ChildExecutor, RejectingChildExecutor, ScriptedChildExecutor,
};
pub use context::{Context, ContextError, ContextRewrite, RewriteReason};
pub use engine::{AgentLoop, EffectGate, Model, ToolHost};
pub use kernel_tools::{
    finish_tool, is_kernel_tool, is_reserved_tenant_name, kernel_name_set, kernel_tool_definitions,
    KERNEL_TOOL_NAMES,
};
pub use manifest::{canonical_hash, LoopManifest, ManifestError, MANIFEST_ORDERING_VERSION};
pub use orchestration::OrchestrationState;
pub use compute::{
    analyze_effects, authorize_effects, compile_compute, compute_observation, eval_compute,
    is_compute_source, run_starlark_pipeline, ComputeProgramV0, EffectSet,
};
pub use program::{
    parse_program, run_compute_program, source_hash, validate_against_tools, ProgramBody,
    ProgramRegistry, ProgramStep, SolProgramV0, StoredProgram,
};
pub use progress::{ProgressMonitor, ProgressNotice, StallKind, TurnFingerprint};
pub use reflect::{ReflectionEpisode, ReflectionLog};
pub use security::{
    compose_policy, ActionBinding, ConfirmationRecord, ConfirmationStatus, PolicyVerdict,
};
pub use tasks::{
    effects_are_parallel_safe, SpawnMode, SpawnRequest, TaskBoard, TaskRecord, TaskResult,
    TaskStatus, MAX_FANOUT, MAX_SESSION_TASKS, MAX_SPAWN_DEPTH,
};
pub use todos::{TodoBoard, TodoItem, TodoStatus};
pub use types::{
    AgentMessage, AgentModelRequestV2, AgentModelResponseV2, AssistantBlock, Budget, BudgetLimits,
    CacheHintsV2, EffectClass, GateDecision, LoopError, LoopEvent, LoopOutcome, ModelRequest,
    ModelResponse, ModelUsage, StopReason, ToolCall, ToolChoiceKindV2, ToolChoiceV2,
    ToolDefinition, ToolError, ToolErrorClass, ToolResult,
};
