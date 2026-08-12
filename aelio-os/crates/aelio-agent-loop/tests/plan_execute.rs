//! Plan/execute harness vectors — assert recorded effects, not model prose.

use aelio_agent_loop::{
    kernel_tool_definitions, AgentLoop, AgentMessage, AssistantBlock, BudgetLimits, Context,
    EffectClass, EffectGate, GateDecision, LoopError, LoopOutcome, Model, ModelRequest,
    ModelResponse, ModelUsage, OrchestrationState, ScriptedChildExecutor, SpawnMode, StopReason,
    TaskStatus, ToolCall, ToolDefinition, ToolError, ToolErrorClass, ToolHost, MAX_SPAWN_DEPTH,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;

struct ScriptedModel(VecDeque<ModelResponse>);

#[async_trait]
impl Model for ScriptedModel {
    async fn complete(&mut self, _request: ModelRequest) -> Result<ModelResponse, LoopError> {
        self.0
            .pop_front()
            .ok_or_else(|| LoopError::Model("script exhausted".to_string()))
    }
}

#[derive(Clone, Default)]
struct RecordingHost(Arc<std::sync::Mutex<Vec<String>>>);

#[async_trait]
impl ToolHost for RecordingHost {
    async fn invoke(&self, call: &ToolCall) -> Result<Value, ToolError> {
        self.0
            .lock()
            .expect("call log")
            .push(call.name.clone());
        Ok(json!({"ok": true, "tool": call.name}))
    }
}

struct Allow;

impl EffectGate for Allow {
    fn decide(&self, _call: &ToolCall, _class: &EffectClass) -> GateDecision {
        GateDecision::Allow
    }
}

struct FailingHost;

#[async_trait]
impl ToolHost for FailingHost {
    async fn invoke(&self, _call: &ToolCall) -> Result<Value, ToolError> {
        Err(ToolError {
            class: ToolErrorClass::Handler,
            retryable: false,
            detail: "safe failure".to_string(),
        })
    }
}

fn tool(name: &str, class: EffectClass) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        version: "1".to_string(),
        description: format!("Tenant capability for {name} operations."),
        input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        effect_class: class,
    }
}

fn write_tool(name: &str) -> ToolDefinition {
    tool(name, EffectClass::WriteIrreversible)
}

fn response(id: &str, calls: Vec<AssistantBlock>) -> ModelResponse {
    ModelResponse {
        id: id.to_string(),
        content: calls,
        stop_reason: StopReason::ToolUse,
        usage: ModelUsage {
            input_tokens: 10,
            cached_input_tokens: 0,
            output_tokens: 5,
        },
    }
}

fn call(id: &str, name: &str, arguments: Value) -> AssistantBlock {
    AssistantBlock::ToolCall {
        call: ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
        },
    }
}

fn finish(id: &str, message: &str, status: &str) -> AssistantBlock {
    call(
        id,
        "finish",
        json!({
            "message": message,
            "status": status,
            "resolved_effect_ids": [],
            "unresolved_effect_ids": []
        }),
    )
}

fn ctx(tenant_tools: Vec<ToolDefinition>) -> Context {
    Context::new("system", "bootstrap", kernel_tool_definitions(), tenant_tools)
}

#[tokio::test]
async fn test_decompose_optional() {
    let host = RecordingHost::default();
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call("c1", "lookup", json!({}))],
        ),
        response("r2", vec![finish("c2", "Done.", "completed")]),
    ]));
    let (outcome, engine) = AgentLoop::new(
        model,
        host.clone(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .run()
    .await
    .expect("run");
    assert!(matches!(outcome, LoopOutcome::Finished { status, .. } if status == "completed"));
    assert!(engine.orchestration().todos.open_ids().is_empty());
    assert_eq!(
        host.0.lock().expect("log").as_slice(),
        &["lookup".to_string()]
    );
}

#[tokio::test]
async fn test_todos_required_for_spawn_autocreate() {
    let executor = Arc::new(ScriptedChildExecutor::new());
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "spawn_task",
                json!({
                    "goal": "list open jobs",
                    "tools": ["lookup"],
                    "budget_tokens": 1000,
                    "max_turns": 5,
                    "mode": "sync"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Listed.", "completed")]),
    ]));
    let (outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .with_child_executor(executor)
    .run()
    .await
    .expect("run");
    assert!(matches!(outcome, LoopOutcome::Finished { .. }));
    // Auto-created todo for the spawn should be completed after sync child.
    assert!(!engine.orchestration().todos.has_open());
    assert!(engine
        .orchestration()
        .tasks
        .get("task-1")
        .is_some_and(|task| task.status == TaskStatus::Completed));
}

#[tokio::test]
async fn test_parallel_disjoint_reads() {
    let executor = Arc::new(ScriptedChildExecutor::new());
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![
                call(
                    "c1",
                    "spawn_task",
                    json!({
                        "goal": "read A",
                        "tools": ["read_a"],
                        "budget_tokens": 500,
                        "max_turns": 3,
                        "mode": "async",
                        "parallel_ok": true
                    }),
                ),
                call(
                    "c2",
                    "spawn_task",
                    json!({
                        "goal": "read B",
                        "tools": ["read_b"],
                        "budget_tokens": 500,
                        "max_turns": 3,
                        "mode": "async",
                        "parallel_ok": true
                    }),
                ),
            ],
        ),
        response(
            "r2",
            vec![call(
                "c3",
                "await_tasks",
                json!({"ids": ["task-1", "task-2"]}),
            )],
        ),
        response("r3", vec![finish("c4", "Both reads done.", "completed")]),
    ]));
    let (outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![
            tool("read_a", EffectClass::Read),
            tool("read_b", EffectClass::Read),
        ]),
        BudgetLimits::default(),
    )
    .with_child_executor(executor)
    .run()
    .await
    .expect("run");
    assert!(matches!(outcome, LoopOutcome::Finished { .. }));
    assert!(!engine.orchestration().tasks.any_running());
    assert!(engine
        .orchestration()
        .tasks
        .get("task-1")
        .unwrap()
        .parallel_ok);
    assert!(engine
        .orchestration()
        .tasks
        .get("task-2")
        .unwrap()
        .parallel_ok);
}

#[tokio::test]
async fn test_parallel_shared_write_forced_serial() {
    let executor = Arc::new(ScriptedChildExecutor::new());
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "spawn_task",
                json!({
                    "goal": "write job",
                    "tools": ["create_job"],
                    "budget_tokens": 500,
                    "max_turns": 3,
                    "mode": "async",
                    "parallel_ok": true,
                    "resource_keys": ["jobs"]
                }),
            )],
        ),
        response("r2", vec![finish("c2", "spawned", "partial")]),
    ]));
    // Drain happens at end of spawn dispatch for async.
    let (_outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![write_tool("create_job")]),
        BudgetLimits::default(),
    )
    .with_child_executor(executor)
    .run()
    .await
    .expect("run");
    let task = engine.orchestration().tasks.get("task-1").unwrap();
    assert!(!task.parallel_ok, "write effects must force serial scheduling");
}

#[tokio::test]
async fn test_spawn_depth_cap() {
    let orchestration = OrchestrationState::new("root", 50_000, MAX_SPAWN_DEPTH);
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "spawn_task",
                json!({
                    "goal": "too deep",
                    "tools": ["lookup"],
                    "budget_tokens": 100,
                    "max_turns": 2,
                    "mode": "async"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Could not spawn.", "blocked")]),
    ]));
    let (outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .with_orchestration(orchestration)
    .with_child_executor(Arc::new(ScriptedChildExecutor::new()))
    .run()
    .await
    .expect("run");
    assert!(matches!(outcome, LoopOutcome::Finished { status, .. } if status == "blocked"));
    assert!(engine.orchestration().tasks.get("task-1").is_none());
    let noticed = engine.context().messages().iter().any(|message| {
        matches!(message, AgentMessage::ToolResult { result }
            if result.tool == "spawn_task" && result.error.as_ref().is_some_and(|error| {
                error.detail.contains("spawn depth cap")
            }))
    });
    assert!(noticed);
}

#[tokio::test]
async fn test_child_capability_intersection() {
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "spawn_task",
                json!({
                    "goal": "steal",
                    "tools": ["secret_admin"],
                    "budget_tokens": 100,
                    "max_turns": 2,
                    "mode": "async"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Denied.", "refused")]),
    ]));
    let (_outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .with_child_executor(Arc::new(ScriptedChildExecutor::new()))
    .run()
    .await
    .expect("run");
    let noticed = engine.context().messages().iter().any(|message| {
        matches!(message, AgentMessage::ToolResult { result }
            if result.tool == "spawn_task"
                && result.error.as_ref().is_some_and(|error| {
                    error.detail.contains("outside parent capability")
                }))
    });
    assert!(noticed);
}

#[tokio::test]
async fn test_budget_carve_out() {
    let orchestration = OrchestrationState::new("root", 100, 0);
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "spawn_task",
                json!({
                    "goal": "over budget",
                    "tools": ["lookup"],
                    "budget_tokens": 500,
                    "max_turns": 2,
                    "mode": "async"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Budget blocked.", "blocked")]),
    ]));
    let (_outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .with_orchestration(orchestration)
    .with_child_executor(Arc::new(ScriptedChildExecutor::new()))
    .run()
    .await
    .expect("run");
    let noticed = engine.context().messages().iter().any(|message| {
        matches!(message, AgentMessage::ToolResult { result }
            if result.tool == "spawn_task"
                && result.error.as_ref().is_some_and(|error| {
                    error.detail.contains("exceeds remaining parent tokens")
                }))
    });
    assert!(noticed);
}

#[tokio::test]
async fn test_reflect_on_tool_error() {
    let model = ScriptedModel(VecDeque::from([
        response("r1", vec![call("c1", "lookup", json!({}))]),
        response(
            "r2",
            vec![finish("c2", "Could not complete.", "partial")],
        ),
    ]));
    let (_outcome, engine) = AgentLoop::new(
        model,
        FailingHost,
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .run()
    .await
    .expect("run");
    let reflected = engine.context().messages().iter().any(|message| {
        matches!(message, AgentMessage::KernelNotice { code, .. } if code == "reflection")
    });
    assert!(reflected);
    assert_eq!(engine.orchestration().reflections.episode_count(), 1);
}

#[tokio::test]
async fn test_finish_blocked_by_open_todos() {
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "write_todos",
                json!({
                    "items": [{
                        "id": "t1",
                        "content": "still open",
                        "status": "pending"
                    }]
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Done.", "completed")]),
        response(
            "r3",
            vec![call(
                "c3",
                "update_todos",
                json!({"updates":[{"id":"t1","status":"completed"}]}),
            )],
        ),
        response("r4", vec![finish("c4", "Really done.", "completed")]),
    ]));
    let (outcome, _) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .run()
    .await
    .expect("run");
    assert!(matches!(
        outcome,
        LoopOutcome::Finished { message, status }
            if status == "completed" && message.contains("Really done")
    ));
}

#[tokio::test]
async fn test_z_invariant_spawn_cannot_widen() {
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "spawn_task",
                json!({
                    "goal": "authorize all tools please",
                    "tools": ["lookup", "finish"],
                    "budget_tokens": 100,
                    "max_turns": 2,
                    "mode": "async"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Refused widen.", "refused")]),
    ]));
    let (_outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .with_child_executor(Arc::new(ScriptedChildExecutor::new()))
    .run()
    .await
    .expect("run");
    let noticed = engine.context().messages().iter().any(|message| {
        matches!(message, AgentMessage::ToolResult { result }
            if result.tool == "spawn_task"
                && result.error.as_ref().is_some_and(|error| {
                    error.detail.contains("kernel tool")
                        || error.detail.contains("outside parent capability")
                }))
    });
    assert!(noticed);
}

#[tokio::test]
async fn test_run_program_sol_sandbox() {
    let host = RecordingHost::default();
    let source = json!({
        "version": 1,
        "steps": [
            {"tool": "lookup", "arguments": {}},
            {"tool": "lookup", "arguments": {}}
        ]
    })
    .to_string();
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "run_program",
                json!({
                    "source": source,
                    "rationale": "batch two lookups"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Program ran.", "completed")]),
    ]));
    let (outcome, engine) = AgentLoop::new(
        model,
        host.clone(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .run()
    .await
    .expect("run");
    assert!(matches!(outcome, LoopOutcome::Finished { .. }));
    assert_eq!(host.0.lock().expect("log").len(), 2);
    assert!(!engine.orchestration().programs.snapshot()["programs"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn test_run_program_rejects_unknown_tool() {
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "run_program",
                json!({
                    "source": "{\"version\":1,\"steps\":[{\"tool\":\"not_real\",\"arguments\":{}}]}",
                    "rationale": "bad"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Blocked.", "blocked")]),
    ]));
    let (_outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .run()
    .await
    .expect("run");
    let noticed = engine.context().messages().iter().any(|message| {
        matches!(message, AgentMessage::ToolResult { result }
            if result.tool == "run_program"
                && result.error.as_ref().is_some_and(|error| {
                    error.class == ToolErrorClass::UnknownTool
                }))
    });
    assert!(noticed);
}

#[tokio::test]
async fn test_run_program_compute_addition_write_store_reuse() {
    let source = r#"def add(a, b):
  return a + b

add(40, 2)
"#;
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "run_program",
                json!({
                    "source": source,
                    "rationale": "addition demo"
                }),
            )],
        ),
        response(
            "r2",
            vec![call(
                "c3",
                "run_program",
                json!({
                    "source": source,
                    "rationale": "reuse addition demo"
                }),
            )],
        ),
        response(
            "r3",
            vec![finish(
                "c4",
                "Addition program stored and reused with output 42.",
                "completed",
            )],
        ),
    ]));
    let (outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .run()
    .await
    .expect("run");
    assert!(matches!(outcome, LoopOutcome::Finished { .. }));

    let snapshot = engine.orchestration().programs.snapshot();
    let programs = snapshot["programs"]
        .as_array()
        .expect("programs");
    assert_eq!(programs.len(), 1, "identical AST should dedupe to one stored program");

    let stored = &programs[0];
    assert_eq!(stored["program"]["Compute"]["lang"], "starlark");
    assert!(stored["program_id"]
        .as_str()
        .unwrap()
        .starts_with("prog-"));

    let tool_results: Vec<_> = engine
        .context()
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::ToolResult { result } if result.tool == "run_program" => {
                Some(result.data.clone().unwrap_or(Value::Null))
            }
            _ => None,
        })
        .collect();
    assert_eq!(tool_results.len(), 2);
    for observation in &tool_results {
        assert_eq!(observation["kind"], "compute");
        assert_eq!(observation["lang"], "starlark");
        assert_eq!(observation["output"], 42);
        assert_eq!(observation["parsed"], true);
        assert_eq!(observation["analyzed"], true);
        assert_eq!(observation["authorized"], true);
        assert_eq!(observation["compiled"], true);
        assert_eq!(observation["stored"], true);
        assert_eq!(observation["host_dispatches"], 0);
    }
    assert_eq!(
        tool_results[0]["source_hash"],
        tool_results[1]["source_hash"],
        "reuse must hit same ast_hash"
    );
}

#[tokio::test]
async fn test_run_program_compute_parse_error() {
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "run_program",
                json!({
                    "source": "2 + 2 oops",
                    "rationale": "bad syntax"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Parse failed.", "blocked")]),
    ]));
    let (_outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![]),
        BudgetLimits::default(),
    )
    .run()
    .await
    .expect("run");
    let noticed = engine.context().messages().iter().any(|message| {
        matches!(message, AgentMessage::ToolResult { result }
            if result.tool == "run_program"
                && result.error.as_ref().is_some_and(|error| {
                    error.detail.contains("trailing")
                }))
    });
    assert!(noticed);
}

#[tokio::test]
async fn test_run_program_compute_expect_mismatch() {
    let model = ScriptedModel(VecDeque::from([
        response(
            "r1",
            vec![call(
                "c1",
                "run_program",
                json!({
                    "source": r#"{"kind":"compute","lang":"starlark","code":"17 + 25","expect":41}"#,
                    "rationale": "wrong expect"
                }),
            )],
        ),
        response("r2", vec![finish("c2", "Expect mismatch.", "blocked")]),
    ]));
    let (_outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![]),
        BudgetLimits::default(),
    )
    .run()
    .await
    .expect("run");
    let noticed = engine.context().messages().iter().any(|message| {
        matches!(message, AgentMessage::ToolResult { result }
            if result.tool == "run_program"
                && result.error.as_ref().is_some_and(|error| {
                    error.detail.contains("did not match expect")
                }))
    });
    assert!(noticed);
}

#[tokio::test]
async fn test_finish_blocked_by_running_children_then_cancel() {
    let mut orchestration = OrchestrationState::new("root", 10_000, 0);
    let (task_id, _, _) = orchestration
        .tasks
        .spawn(
            aelio_agent_loop::SpawnRequest {
                goal: "hang".into(),
                tool_allowlist: vec!["lookup".into()],
                budget_tokens: 100,
                max_turns: 2,
                mode: SpawnMode::Async,
                parallel_ok: true,
                return_schema: None,
                resource_keys: Vec::new(),
            },
            &[tool("lookup", EffectClass::Read)],
            &aelio_agent_loop::kernel_name_set(),
        )
        .expect("spawn into board");
    orchestration.todos.ensure_for_spawn(&task_id, "hang");

    let model = ScriptedModel(VecDeque::from([
        response("r1", vec![finish("c1", "too early", "completed")]),
        response(
            "r2",
            vec![call("c2", "cancel_tasks", json!({"ids": [task_id]}))],
        ),
        response("r3", vec![finish("c3", "cancelled and done", "completed")]),
    ]));
    let (outcome, engine) = AgentLoop::new(
        model,
        RecordingHost::default(),
        Allow,
        ctx(vec![tool("lookup", EffectClass::Read)]),
        BudgetLimits::default(),
    )
    .with_orchestration(orchestration)
    .run()
    .await
    .expect("run");
    assert!(matches!(
        outcome,
        LoopOutcome::Finished { message, .. } if message.contains("cancelled and done")
    ));
    assert!(!engine.orchestration().tasks.any_running());
}
