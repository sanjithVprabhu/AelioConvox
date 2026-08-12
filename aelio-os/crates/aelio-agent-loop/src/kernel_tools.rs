//! Always-present kernel tool definitions for the plan/execute loop.

use crate::{EffectClass, ToolDefinition};
use serde_json::json;
use std::collections::HashSet;

pub const KERNEL_TOOL_NAMES: &[&str] = &[
    "finish",
    "write_todos",
    "update_todos",
    "spawn_task",
    "check_tasks",
    "await_tasks",
    "cancel_tasks",
    "run_program",
];

pub fn kernel_name_set() -> HashSet<&'static str> {
    KERNEL_TOOL_NAMES.iter().copied().collect()
}

pub fn is_kernel_tool(name: &str) -> bool {
    KERNEL_TOOL_NAMES.contains(&name)
}

pub fn is_reserved_tenant_name(name: &str) -> bool {
    is_kernel_tool(name)
}

/// Kernel tools first (stable order), ready to pass as `kernel_tools` into [`Context::new`].
pub fn kernel_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        finish_tool(),
        write_todos_tool(),
        update_todos_tool(),
        spawn_task_tool(),
        check_tasks_tool(),
        await_tasks_tool(),
        cancel_tasks_tool(),
        run_program_tool(),
    ]
}

pub fn finish_tool() -> ToolDefinition {
    ToolDefinition {
        name: "finish".into(),
        version: "1".into(),
        description: "Finish the current request with a customer-facing message and explicit status. This must be called alone.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "message": {"type": "string", "minLength": 1},
                "status": {"type": "string", "enum": ["completed", "partial", "blocked", "refused"]},
                "resolved_effect_ids": {"type": "array", "items": {"type": "string"}},
                "unresolved_effect_ids": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["message", "status", "resolved_effect_ids", "unresolved_effect_ids"],
            "additionalProperties": false
        }),
        effect_class: EffectClass::Read,
    }
}

fn write_todos_tool() -> ToolDefinition {
    ToolDefinition {
        name: "write_todos".into(),
        version: "1".into(),
        description: "Replace the todo board with a structured multi-step plan. Use for complex goals before spawning sub-tasks.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 64,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "minLength": 1, "maxLength": 64},
                            "content": {"type": "string", "minLength": 1, "maxLength": 512},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]}
                        },
                        "required": ["id", "content", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["items"],
            "additionalProperties": false
        }),
        effect_class: EffectClass::Read,
    }
}

fn update_todos_tool() -> ToolDefinition {
    ToolDefinition {
        name: "update_todos".into(),
        version: "1".into(),
        description: "Update status of existing todo items as work completes or is cancelled.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "updates": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "minLength": 1},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]}
                        },
                        "required": ["id", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["updates"],
            "additionalProperties": false
        }),
        effect_class: EffectClass::Read,
    }
}

fn spawn_task_tool() -> ToolDefinition {
    ToolDefinition {
        name: "spawn_task".into(),
        version: "1".into(),
        description: "Spawn a scoped sub-harness for an isolated goal. Child tools must be a subset of parent capability.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "goal": {"type": "string", "minLength": 1, "maxLength": 4096},
                "tools": {
                    "type": "array",
                    "minItems": 1,
                    "items": {"type": "string"}
                },
                "budget_tokens": {"type": "integer", "minimum": 1},
                "max_turns": {"type": "integer", "minimum": 1, "maximum": 120},
                "mode": {"type": "string", "enum": ["sync", "async"]},
                "parallel_ok": {"type": "boolean"},
                "resource_keys": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "return_schema": {}
            },
            "required": ["goal", "tools", "budget_tokens", "max_turns", "mode"],
            "additionalProperties": false
        }),
        effect_class: EffectClass::Read,
    }
}

fn check_tasks_tool() -> ToolDefinition {
    ToolDefinition {
        name: "check_tasks".into(),
        version: "1".into(),
        description: "Poll outstanding sub-harness tasks without blocking.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        effect_class: EffectClass::Read,
    }
}

fn await_tasks_tool() -> ToolDefinition {
    ToolDefinition {
        name: "await_tasks".into(),
        version: "1".into(),
        description: "Block until the listed sub-harness tasks complete; drains results into the parent context.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "ids": {
                    "type": "array",
                    "minItems": 1,
                    "items": {"type": "string"}
                }
            },
            "required": ["ids"],
            "additionalProperties": false
        }),
        effect_class: EffectClass::Read,
    }
}

fn cancel_tasks_tool() -> ToolDefinition {
    ToolDefinition {
        name: "cancel_tasks".into(),
        version: "1".into(),
        description: "Cancel running sub-harnesses and their descendants; returns unused budget to the parent.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "ids": {
                    "type": "array",
                    "minItems": 1,
                    "items": {"type": "string"}
                }
            },
            "required": ["ids"],
            "additionalProperties": false
        }),
        effect_class: EffectClass::Read,
    }
}

fn run_program_tool() -> ToolDefinition {
    ToolDefinition {
        name: "run_program".into(),
        version: "1".into(),
        description: "Execute a reusable program. Dialects: (1) Starlark-surface compute — write `def add(a, b):\\n  return a + b\\n\\nadd(40, 2)` (or JSON {\"kind\":\"compute\",\"lang\":\"starlark\",\"code\":\"...\",\"expect\":42}); pipeline parse→analyze→authorize→execute; store by AST hash; Observation includes output/program_id; (2) Sol JSON steps[] — tool batches under the effect gate. Prefer Starlark for arithmetic; Sol for tool recipes.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "source": {"type": "string", "minLength": 1},
                "rationale": {"type": "string"}
            },
            "required": ["source", "rationale"],
            "additionalProperties": false
        }),
        effect_class: EffectClass::WriteReversible,
    }
}
