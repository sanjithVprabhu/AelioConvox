use crate::child::{ChildExecutionRequest, ChildExecutor, RejectingChildExecutor};
use crate::kernel_tools::{is_kernel_tool, kernel_name_set};
use crate::orchestration::OrchestrationState;
use crate::compute::{
    compute_observation, is_compute_source, run_starlark_pipeline,
};
use crate::program::{parse_program, source_hash, validate_against_tools};
use crate::tasks::{SpawnMode, SpawnRequest, TaskBoard, TaskResult, TaskStatus};
use crate::todos::{TodoItem, TodoStatus};
use crate::{
    AssistantBlock, Budget, BudgetLimits, Context, EffectClass, GateDecision, LoopError, LoopEvent,
    LoopOutcome, ModelRequest, ModelResponse, ProgressMonitor, ToolCall, ToolError, ToolErrorClass,
    ToolResult, TurnFingerprint,
};
use async_trait::async_trait;
use futures_util::future::join_all;
use futures_util::FutureExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

#[async_trait]
pub trait Model: Send {
    async fn complete(&mut self, request: ModelRequest) -> Result<ModelResponse, LoopError>;
}

#[async_trait]
pub trait ToolHost: Send + Sync {
    /// Cooperative cancellation is checked before model calls and before each sequential
    /// effect dispatch. An invocation already inside the host may land, but no later call starts.
    async fn cancelled(&self) -> bool {
        false
    }

    /// Reserve/check host-owned resources for the entire response before any call is dispatched.
    /// Implementations use this for persistent rate limits, circuit breakers, and SDK readiness.
    async fn preflight(&self, _calls: &[ToolCall]) -> Result<(), ToolError> {
        Ok(())
    }

    async fn invoke(&self, call: &ToolCall) -> Result<Value, ToolError>;
}

pub trait EffectGate: Send + Sync {
    fn decide(&self, call: &ToolCall, class: &EffectClass) -> GateDecision;
}

pub struct AgentLoop<M, H, G> {
    model: M,
    host: H,
    gate: G,
    context: Context,
    limits: BudgetLimits,
    budget: Budget,
    progress: ProgressMonitor,
    events: Vec<LoopEvent>,
    started: Instant,
    orchestration: OrchestrationState,
    child_executor: Arc<dyn ChildExecutor>,
}

impl<M, H, G> AgentLoop<M, H, G>
where
    M: Model,
    H: ToolHost,
    G: EffectGate,
{
    pub fn new(model: M, host: H, gate: G, context: Context, limits: BudgetLimits) -> Self {
        let remaining = limits.max_total_tokens.saturating_sub(limits.reserve_tokens);
        Self {
            model,
            host,
            gate,
            context,
            limits,
            budget: Budget::default(),
            progress: ProgressMonitor::default(),
            events: Vec::new(),
            started: Instant::now(),
            orchestration: OrchestrationState::new("root", remaining, 0),
            child_executor: Arc::new(RejectingChildExecutor),
        }
    }

    pub fn with_orchestration(mut self, orchestration: OrchestrationState) -> Self {
        self.orchestration = orchestration;
        self
    }

    pub fn with_child_executor(mut self, executor: Arc<dyn ChildExecutor>) -> Self {
        self.child_executor = executor;
        self
    }

    pub fn with_depth(mut self, depth: u32) -> Self {
        self.orchestration.tasks.depth = depth;
        self
    }

    pub async fn run(mut self) -> Result<(LoopOutcome, Self), LoopError> {
        let tool_definitions: HashMap<String, crate::ToolDefinition> = self
            .context
            .tools()
            .iter()
            .map(|tool| (tool.name.clone(), tool.clone()))
            .collect();
        let mut overflow_rewrite_used = false;

        loop {
            if self.host.cancelled().await {
                return Ok((cancelled_outcome(), self));
            }
            if let Some(outcome) = self.hard_terminator() {
                return Ok((outcome, self));
            }

            let response = match self.model.complete(self.context.request()).await {
                Ok(response) => response,
                Err(LoopError::ContextOverflow(detail)) if !overflow_rewrite_used => {
                    let current_messages = self.context.messages().len();
                    let target = self
                        .limits
                        .overflow_compaction_messages
                        .min(current_messages.saturating_div(2).max(2));
                    let rewrite = self
                        .context
                        .compact(target)
                        .map_err(|error| LoopError::Context(error.to_string()))?;
                    let Some(rewrite) = rewrite else {
                        return Ok((context_overflow_outcome(&detail), self));
                    };
                    overflow_rewrite_used = true;
                    self.events.push(LoopEvent::ContextRewritten {
                        reason: "provider_context_overflow".to_string(),
                        before_hash: rewrite.before_hash,
                        after_hash: rewrite.after_hash,
                    });
                    continue;
                }
                Err(LoopError::ContextOverflow(detail)) => {
                    return Ok((context_overflow_outcome(&detail), self));
                }
                Err(error) => return Err(error),
            };
            self.budget.charge(response.usage);
            self.events.push(LoopEvent::ModelCompleted {
                response_id: response.id.clone(),
            });
            let calls = response_calls(&response);
            self.context.append_assistant(response);
            if self.host.cancelled().await {
                for call in &calls {
                    self.context.append_tool_result(ToolResult::error(
                        call,
                        ToolError {
                            class: ToolErrorClass::Handler,
                            retryable: false,
                            detail: "turn cancelled before tool dispatch completed".to_string(),
                        },
                    ));
                }
                return Ok((cancelled_outcome(), self));
            }

            if calls.is_empty() {
                self.budget.protocol_violations = self.budget.protocol_violations.saturating_add(1);
                self.context.append_notice(
                    "must_call_finish",
                    "Plain assistant text does not complete the turn; call finish.",
                );
                self.events.push(LoopEvent::ProtocolViolation {
                    count: self.budget.protocol_violations,
                    code: "must_call_finish".to_string(),
                });
                continue;
            }

            let finish_calls: Vec<&ToolCall> =
                calls.iter().filter(|call| call.name == "finish").collect();
            if !finish_calls.is_empty() {
                if calls.len() != 1 || finish_calls.len() != 1 {
                    let call = finish_calls[0];
                    self.context.append_tool_result(ToolResult::error(
                        call,
                        ToolError {
                            class: ToolErrorClass::MixedFinishAndActions,
                            retryable: false,
                            detail: "finish must be the only tool call in a response".to_string(),
                        },
                    ));
                    continue;
                }
                match parse_finish(finish_calls[0]) {
                    Ok(outcome) => match self.completion_gate(&outcome) {
                        Ok(()) => {
                            let status = match &outcome {
                                LoopOutcome::Finished { status, .. } => status.clone(),
                                _ => "completed".to_string(),
                            };
                            self.context.append_tool_result(ToolResult::ok(
                                finish_calls[0],
                                json!({
                                    "accepted": true,
                                    "status": status,
                                }),
                            ));
                            return Ok((outcome, self));
                        }
                        Err(error) => {
                            self.context
                                .append_tool_result(ToolResult::error(finish_calls[0], error));
                            continue;
                        }
                    },
                    Err(error) => {
                        self.context
                            .append_tool_result(ToolResult::error(finish_calls[0], error));
                        continue;
                    }
                }
            }

            let mut call_ids = HashSet::with_capacity(calls.len());
            if calls.iter().any(|call| !call_ids.insert(call.id.clone())) {
                self.budget.protocol_violations = self.budget.protocol_violations.saturating_add(1);
                for call in calls {
                    self.context.append_tool_result(ToolResult::error(
                        &call,
                        ToolError {
                            class: ToolErrorClass::InvalidArguments,
                            retryable: false,
                            detail: "tool call IDs must be unique within one model response"
                                .to_string(),
                        },
                    ));
                }
                self.events.push(LoopEvent::ProtocolViolation {
                    count: self.budget.protocol_violations,
                    code: "duplicate_tool_call_id".to_string(),
                });
                continue;
            }

            let mut fingerprint = TurnFingerprint {
                calls: Vec::new(),
                result_hashes: Vec::new(),
                successful_non_read_effects: 0,
            };
            let mut prepared = Vec::with_capacity(calls.len());
            let mut rejected = HashMap::new();
            let mut confirmation = None;

            let mut missing_fields = BTreeSet::new();
            for call in &calls {
                if let Some(tool) = tool_definitions.get(&call.name) {
                    missing_fields.extend(missing_required_arguments(
                        &tool.input_schema,
                        &call.arguments,
                    ));
                }
            }
            if !missing_fields.is_empty() {
                let missing_fields = missing_fields.into_iter().collect::<Vec<_>>();
                let readable = missing_fields
                    .iter()
                    .map(|field| field.replace(['_', '.', '-'], " "))
                    .collect::<Vec<_>>()
                    .join(", ");
                for call in &calls {
                    self.context.append_tool_result(ToolResult::error(
                        call,
                        ToolError {
                            class: ToolErrorClass::InvalidArguments,
                            retryable: true,
                            detail: format!(
                                "missing required arguments: {}",
                                missing_fields.join(", ")
                            ),
                        },
                    ));
                }
                self.context.append_notice(
                    "missing_tool_arguments",
                    format!("No tools were dispatched. Required values: {readable}"),
                );
                return Ok((
                    LoopOutcome::WaitingForInput {
                        message: format!(
                            "I need a little more information to continue: {readable}."
                        ),
                        pending_calls: calls,
                        missing_fields,
                    },
                    self,
                ));
            }

            for call in &calls {
                self.budget.tool_calls = self.budget.tool_calls.saturating_add(1);
                let args = serde_json::to_vec(&call.arguments)
                    .map_err(|error| LoopError::Context(error.to_string()))?;
                fingerprint
                    .calls
                    .push((call.name.clone(), blake3::hash(&args).to_hex().to_string()));
                let Some(class) = tool_definitions
                    .get(&call.name)
                    .map(|tool| &tool.effect_class)
                else {
                    rejected.insert(
                        call.id.clone(),
                        ToolError {
                            class: ToolErrorClass::UnknownTool,
                            retryable: false,
                            detail: "tool is not present in the pinned manifest".to_string(),
                        },
                    );
                    continue;
                };
                match self.gate.decide(call, class) {
                    GateDecision::Allow => prepared.push((call.clone(), class.clone())),
                    GateDecision::Deny { reason } => {
                        self.events.push(LoopEvent::ToolDenied {
                            call_id: call.id.clone(),
                            reason: reason.clone(),
                        });
                        rejected.insert(
                            call.id.clone(),
                            ToolError {
                                class: ToolErrorClass::NotAuthorized,
                                retryable: false,
                                detail: reason,
                            },
                        );
                    }
                    GateDecision::Confirm { reason } => {
                        confirmation = Some(reason);
                    }
                }
            }

            if let Some(message) = confirmation {
                return Ok((
                    LoopOutcome::WaitingForUser {
                        message,
                        pending_calls: calls,
                    },
                    self,
                ));
            }

            if !rejected.is_empty() {
                let mut rejected_ids: Vec<String> = rejected.keys().cloned().collect();
                rejected_ids.sort();
                for call in calls {
                    let error = rejected.remove(&call.id).unwrap_or_else(|| ToolError {
                        class: ToolErrorClass::NotAuthorized,
                        retryable: true,
                        detail: format!(
                            "tool batch was not dispatched because these calls failed preflight: {}",
                            rejected_ids.join(", ")
                        ),
                    });
                    self.context
                        .append_tool_result(ToolResult::error(&call, error));
                }
                if let Some(notice) = self.progress.observe(fingerprint) {
                    self.record_progress_notice(&notice);
                    if notice.strike >= 3 {
                        return Ok((no_progress_outcome(), self));
                    }
                }
                continue;
            }

            let host_calls: Vec<ToolCall> = prepared
                .iter()
                .filter(|(call, _)| !is_kernel_tool(&call.name))
                .map(|(call, _)| call.clone())
                .collect();
            if !host_calls.is_empty() {
                if let Err(error) = self.host.preflight(&host_calls).await {
                    for call in calls {
                        self.events.push(LoopEvent::ToolDenied {
                            call_id: call.id.clone(),
                            reason: error.detail.clone(),
                        });
                        self.context
                            .append_tool_result(ToolResult::error(&call, error.clone()));
                    }
                    if let Some(notice) = self.progress.observe(fingerprint) {
                        self.record_progress_notice(&notice);
                        if notice.strike >= 3 {
                            return Ok((no_progress_outcome(), self));
                        }
                    }
                    continue;
                }
            }

            // Kernel tools mutate orchestration state and must stay sequential. Pure host reads
            // may still run concurrently within max_parallel_reads.
            let all_reads = prepared
                .iter()
                .all(|(_, class)| class == &EffectClass::Read);
            let has_kernel = prepared.iter().any(|(call, _)| is_kernel_tool(&call.name));
            let results = if all_reads && !has_kernel {
                for (call, _) in &prepared {
                    self.events.push(LoopEvent::ToolDispatched {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                    });
                }
                let mut results = Vec::with_capacity(prepared.len());
                for batch in prepared.chunks(self.limits.max_parallel_reads.max(1)) {
                    results.extend(
                        join_all(batch.iter().map(|(call, _)| {
                            invoke_host(
                                &self.host,
                                call,
                                self.limits.max_result_bytes,
                                self.limits.tool_timeout,
                            )
                        }))
                        .await,
                    );
                }
                results
            } else {
                let mut results = Vec::with_capacity(prepared.len());
                for (call, _) in &prepared {
                    if self.host.cancelled().await {
                        results.push(ToolResult::error(
                            call,
                            ToolError {
                                class: ToolErrorClass::NotAuthorized,
                                retryable: false,
                                detail: "conversation cancellation prevented this dispatch"
                                    .to_string(),
                            },
                        ));
                    } else {
                        self.events.push(LoopEvent::ToolDispatched {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                        });
                        results.push(self.dispatch_one(call).await);
                    }
                }
                results
            };

            let mut spawned_async = Vec::new();
            for ((call, class), result) in prepared.iter().zip(results) {
                if let Some(value) = &result.data {
                    let bytes = serde_json::to_vec(value)
                        .map_err(|error| LoopError::Context(error.to_string()))?;
                    fingerprint
                        .result_hashes
                        .push(blake3::hash(&bytes).to_hex().to_string());
                    if class != &EffectClass::Read {
                        fingerprint.successful_non_read_effects =
                            fingerprint.successful_non_read_effects.saturating_add(1);
                    }
                    if call.name == "spawn_task" {
                        if let Some(task_id) = value.get("task_id").and_then(Value::as_str) {
                            if value.get("mode").and_then(Value::as_str) == Some("async")
                                && value.get("status").and_then(Value::as_str) == Some("running")
                            {
                                spawned_async.push(task_id.to_string());
                            }
                        }
                    }
                }
                let failed = result.error.is_some();
                self.events.push(LoopEvent::ToolCompleted {
                    call_id: call.id.clone(),
                    ok: !failed,
                });
                if let Some(error) = &result.error {
                    self.emit_reflection_for_tool(&call.name, &error.detail);
                }
                self.context.append_tool_result(result);
            }

            // Drain async children spawned this turn at end of dispatch (append-only).
            if !spawned_async.is_empty() {
                let drained = self.execute_tasks(Some(&spawned_async)).await;
                if !drained.is_empty() {
                    self.context.append_notice(
                        "pending_tasks",
                        serde_json::to_string(&json!({
                            "drained": drained,
                            "pending_tasks": self.orchestration.tasks.running_ids(),
                        }))
                        .unwrap_or_else(|_| "pending_tasks updated".into()),
                    );
                }
            }

            if let Some(notice) = self.progress.observe(fingerprint) {
                self.record_progress_notice(&notice);
                if notice.strike >= 3 {
                    return Ok((no_progress_outcome(), self));
                }
            }
        }
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    pub fn events(&self) -> &[LoopEvent] {
        &self.events
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn orchestration(&self) -> &OrchestrationState {
        &self.orchestration
    }

    pub fn into_orchestration(self) -> OrchestrationState {
        self.orchestration
    }

    fn completion_gate(&self, outcome: &LoopOutcome) -> Result<(), ToolError> {
        let LoopOutcome::Finished { status, .. } = outcome else {
            return Ok(());
        };
        if self.orchestration.tasks.any_running() {
            return Err(ToolError {
                class: ToolErrorClass::CompletionRejected,
                retryable: true,
                detail: format!(
                    "pending_tasks: {}",
                    self.orchestration.tasks.running_ids().join(", ")
                ),
            });
        }
        if status == "completed" && self.orchestration.todos.has_open() {
            return Err(ToolError {
                class: ToolErrorClass::CompletionRejected,
                retryable: true,
                detail: format!(
                    "open_todos: {}",
                    self.orchestration.todos.open_ids().join(", ")
                ),
            });
        }
        if status == "completed"
            && self
                .context
                .messages()
                .iter()
                .rev()
                .find_map(|message| match message {
                    crate::AgentMessage::ToolResult { result } => Some(result.error.is_some()),
                    _ => None,
                })
                .unwrap_or(false)
        {
            return Err(ToolError {
                class: ToolErrorClass::CompletionRejected,
                retryable: true,
                detail: "completed status is inconsistent with the latest failed tool result; acknowledge the unresolved action".to_string(),
            });
        }
        Ok(())
    }

    async fn dispatch_one(&mut self, call: &ToolCall) -> ToolResult {
        if is_kernel_tool(&call.name) {
            return match self.invoke_kernel(call).await {
                Ok(value) => ToolResult::bounded(call, Ok(value), self.limits.max_result_bytes),
                Err(error) => ToolResult::error(call, error),
            };
        }
        invoke_host(
            &self.host,
            call,
            self.limits.max_result_bytes,
            self.limits.tool_timeout,
        )
        .await
    }

    async fn invoke_kernel(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        match call.name.as_str() {
            "write_todos" => self.kernel_write_todos(call),
            "update_todos" => self.kernel_update_todos(call),
            "spawn_task" => self.kernel_spawn_task(call).await,
            "check_tasks" => Ok(self.orchestration.tasks.check()),
            "await_tasks" => self.kernel_await_tasks(call).await,
            "cancel_tasks" => self.kernel_cancel_tasks(call),
            "run_program" => self.kernel_run_program(call).await,
            other => Err(ToolError {
                class: ToolErrorClass::UnknownTool,
                retryable: false,
                detail: format!("kernel tool `{other}` is not handled"),
            }),
        }
    }

    fn kernel_write_todos(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Item {
            id: String,
            content: String,
            status: String,
        }
        #[derive(Deserialize)]
        struct Args {
            items: Vec<Item>,
        }
        let args: Args = serde_json::from_value(call.arguments.clone()).map_err(args_error)?;
        let items = args
            .items
            .into_iter()
            .map(|item| {
                let status = TodoStatus::parse(&item.status).ok_or_else(|| ToolError {
                    class: ToolErrorClass::InvalidArguments,
                    retryable: true,
                    detail: format!("invalid todo status `{}`", item.status),
                })?;
                Ok(TodoItem {
                    id: item.id,
                    content: item.content,
                    status,
                })
            })
            .collect::<Result<Vec<_>, ToolError>>()?;
        self.orchestration
            .todos
            .write(items)
            .map_err(|detail| ToolError {
                class: ToolErrorClass::InvalidArguments,
                retryable: true,
                detail,
            })
    }

    fn kernel_update_todos(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Update {
            id: String,
            status: String,
        }
        #[derive(Deserialize)]
        struct Args {
            updates: Vec<Update>,
        }
        let args: Args = serde_json::from_value(call.arguments.clone()).map_err(args_error)?;
        let updates = args
            .updates
            .into_iter()
            .map(|item| {
                let status = TodoStatus::parse(&item.status).ok_or_else(|| ToolError {
                    class: ToolErrorClass::InvalidArguments,
                    retryable: true,
                    detail: format!("invalid todo status `{}`", item.status),
                })?;
                Ok((item.id, status))
            })
            .collect::<Result<Vec<_>, ToolError>>()?;
        self.orchestration
            .todos
            .update(updates)
            .map_err(|detail| ToolError {
                class: ToolErrorClass::InvalidArguments,
                retryable: true,
                detail,
            })
    }

    async fn kernel_spawn_task(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Args {
            goal: String,
            tools: Vec<String>,
            budget_tokens: u64,
            max_turns: u32,
            mode: String,
            #[serde(default)]
            parallel_ok: bool,
            #[serde(default)]
            resource_keys: Vec<String>,
            #[serde(default)]
            return_schema: Option<Value>,
        }
        let args: Args = serde_json::from_value(call.arguments.clone()).map_err(args_error)?;
        let mode = SpawnMode::parse(&args.mode).ok_or_else(|| ToolError {
            class: ToolErrorClass::InvalidArguments,
            retryable: true,
            detail: "mode must be sync or async".into(),
        })?;
        let request = SpawnRequest {
            goal: args.goal.clone(),
            tool_allowlist: args.tools,
            budget_tokens: args.budget_tokens,
            max_turns: args.max_turns,
            mode,
            parallel_ok: args.parallel_ok,
            return_schema: args.return_schema,
            resource_keys: args.resource_keys,
        };
        let names = kernel_name_set();
        let (task_id, mode, parallel_ok) = self
            .orchestration
            .tasks
            .spawn(request, self.context.tools(), &names)
            .map_err(|detail| ToolError {
                class: ToolErrorClass::NotAuthorized,
                retryable: false,
                detail,
            })?;
        self.orchestration
            .todos
            .ensure_for_spawn(&task_id, &args.goal);
        if mode == SpawnMode::Sync {
            let results = self.execute_tasks(Some(&[task_id.clone()])).await;
            let result = results.into_iter().next().unwrap_or(json!({
                "task_id": task_id,
                "status": "failed",
                "summary": "sync child produced no result",
            }));
            return Ok(result);
        }
        Ok(json!({
            "task_id": task_id,
            "status": "running",
            "mode": "async",
            "parallel_ok": parallel_ok,
            "pending_tasks": self.orchestration.tasks.running_ids(),
        }))
    }

    async fn kernel_await_tasks(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Args {
            ids: Vec<String>,
        }
        let args: Args = serde_json::from_value(call.arguments.clone()).map_err(args_error)?;
        if args.ids.is_empty() {
            return Err(ToolError {
                class: ToolErrorClass::InvalidArguments,
                retryable: true,
                detail: "await_tasks requires at least one id".into(),
            });
        }
        let drained = self.execute_tasks(Some(&args.ids)).await;
        Ok(json!({
            "results": drained,
            "pending_tasks": self.orchestration.tasks.running_ids(),
            "todos": self.orchestration.todos.snapshot(),
        }))
    }

    fn kernel_cancel_tasks(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Args {
            ids: Vec<String>,
        }
        let args: Args = serde_json::from_value(call.arguments.clone()).map_err(args_error)?;
        let value = self.orchestration.tasks.cancel_subtree(&args.ids);
        for id in &args.ids {
            self.orchestration.todos.mark(id, TodoStatus::Cancelled);
        }
        Ok(value)
    }

    async fn kernel_run_program(&mut self, call: &ToolCall) -> Result<Value, ToolError> {
        #[derive(Deserialize)]
        struct Args {
            source: Value,
            rationale: String,
        }
        let args: Args = serde_json::from_value(call.arguments.clone()).map_err(args_error)?;
        let source = match args.source {
            Value::String(text) => text,
            other => other.to_string(),
        };
        if args.rationale.trim().is_empty() {
            return Err(ToolError {
                class: ToolErrorClass::InvalidArguments,
                retryable: true,
                detail: "rationale must be a non-empty string".to_string(),
            });
        }

        // Starlark-surface A-12 spike: parse → analyze → authorize → execute → store.
        if is_compute_source(&source) {
            let (program, run) = run_starlark_pipeline(&source, &args.rationale)?;
            let hash = program.ast_hash.clone();
            let stored = self
                .orchestration
                .programs
                .store_compute(program.clone(), hash.clone());
            return Ok(compute_observation(
                &program,
                &hash,
                &stored.program_id,
                &run,
            ));
        }

        let program = parse_program(&source, &args.rationale)?;
        let names = kernel_name_set();
        let steps = validate_against_tools(&program, self.context.tools(), &names)?;
        // Program steps are host tools; they must be reserved like a normal batch or
        // RuntimeToolHost rejects with "not durably reserved before dispatch".
        if let Err(error) = self.host.preflight(&steps).await {
            return Err(error);
        }
        let hash = source_hash(&source);
        let mut step_results = Vec::new();
        let mut effects = Vec::new();
        for step in steps {
            match self.gate.decide(
                &step,
                &self
                    .context
                    .tools()
                    .iter()
                    .find(|tool| tool.name == step.name)
                    .map(|tool| tool.effect_class.clone())
                    .unwrap_or(EffectClass::Read),
            ) {
                GateDecision::Allow => {}
                GateDecision::Deny { reason } | GateDecision::Confirm { reason } => {
                    return Err(ToolError {
                        class: ToolErrorClass::NotAuthorized,
                        retryable: false,
                        detail: format!(
                            "run_program step `{}` was not authorized: {reason}",
                            step.name
                        ),
                    });
                }
            }
            let result = invoke_host(
                &self.host,
                &step,
                self.limits.max_result_bytes,
                self.limits.tool_timeout,
            )
            .await;
            if let Some(error) = result.error {
                return Err(error);
            }
            effects.push(json!({
                "tool": step.name,
                "call_id": step.id,
            }));
            step_results.push(result.data.unwrap_or(Value::Null));
        }
        let stored = self.orchestration.programs.store(program, hash);
        Ok(json!({
            "kind": "sol",
            "program_id": stored.program_id,
            "source_hash": stored.source_hash,
            "steps": step_results,
            "effects_performed": effects,
            "stored": true,
            "reusable": true,
        }))
    }

    async fn execute_tasks(&mut self, ids: Option<&[String]>) -> Vec<Value> {
        let running = self.orchestration.tasks.take_running(ids);
        if running.is_empty() {
            return Vec::new();
        }
        let (parallel, serial) = TaskBoard::schedule_wave(&running);
        let mut out = Vec::new();
        if !parallel.is_empty() {
            let executor = Arc::clone(&self.child_executor);
            let futures = parallel.into_iter().map(|task| {
                let executor = Arc::clone(&executor);
                async move {
                    executor
                        .execute(ChildExecutionRequest { task })
                        .await
                }
            });
            for result in join_all(futures).await {
                out.push(self.commit_task_result(result));
            }
        }
        for task in serial {
            let result = self
                .child_executor
                .execute(ChildExecutionRequest { task })
                .await;
            out.push(self.commit_task_result(result));
        }
        out
    }

    fn commit_task_result(&mut self, result: TaskResult) -> Value {
        let task_id = result.task_id.clone();
        let status = result.status;
        let value = json!({
            "task_id": result.task_id,
            "status": status,
            "summary": result.summary,
            "data": result.data,
            "effects_performed": result.effects_performed,
            "tokens_used": result.tokens_used,
        });
        let _ = self.orchestration.tasks.complete(result);
        let todo_status = match status {
            TaskStatus::Completed => TodoStatus::Completed,
            TaskStatus::Cancelled => TodoStatus::Cancelled,
            TaskStatus::Failed | TaskStatus::Exhausted => TodoStatus::Cancelled,
            TaskStatus::Running => TodoStatus::InProgress,
        };
        self.orchestration.todos.mark(&task_id, todo_status);
        value
    }

    fn emit_reflection_for_tool(&mut self, tool: &str, detail: &str) {
        let hash = context_messages_hash(&self.context);
        if let Some(payload) = self
            .orchestration
            .reflections
            .maybe_reflect_tool_error(tool, detail, &hash)
        {
            self.context.append_notice(
                "reflection",
                serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
            );
        }
    }

    fn record_progress_notice(&mut self, notice: &crate::ProgressNotice) {
        self.events.push(LoopEvent::ProgressNotice {
            kind: format!("{:?}", notice.kind),
        });
        self.context
            .append_notice("progress_stall", notice.detail.clone());
        let hash = context_messages_hash(&self.context);
        if let Some(payload) = self.orchestration.reflections.maybe_reflect_stall(
            &format!("{:?}", notice.kind),
            &notice.detail,
            &hash,
        ) {
            self.context.append_notice(
                "reflection",
                serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
            );
        }
    }

    fn hard_terminator(&self) -> Option<LoopOutcome> {
        let reason = if self.budget.turns >= self.limits.max_turns {
            Some("turns")
        } else if self.budget.tool_calls >= self.limits.max_tool_calls {
            Some("tool_calls")
        } else if self.budget.protocol_violations >= self.limits.max_protocol_violations {
            Some("protocol_violations")
        } else if self
            .budget
            .total_tokens()
            .saturating_add(self.limits.reserve_tokens)
            >= self.limits.max_total_tokens
        {
            Some("tokens")
        } else if self.started.elapsed() >= self.limits.wall_clock {
            Some("wall_clock")
        } else {
            None
        }?;
        Some(LoopOutcome::Exhausted {
            reason: reason.to_string(),
            message: "I wasn't able to finish that request within the safe execution limits."
                .to_string(),
        })
    }
}

fn context_messages_hash(context: &Context) -> String {
    serde_json::to_vec(context.messages())
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
        .unwrap_or_else(|_| "unhashable".into())
}

fn missing_required_arguments(schema: &Value, arguments: &Value) -> Vec<String> {
    let Some(required) = schema.get("required").and_then(Value::as_array) else {
        return Vec::new();
    };
    let Some(arguments) = arguments.as_object() else {
        return required
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    };
    required
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| {
            !arguments.get(*name).is_some_and(|value| {
                !value.is_null() && value.as_str().is_none_or(|s| !s.trim().is_empty())
            })
        })
        .map(str::to_string)
        .collect()
}

fn context_overflow_outcome(detail: &str) -> LoopOutcome {
    LoopOutcome::Exhausted {
        reason: "context_overflow".to_string(),
        message: if detail.is_empty() {
            "This conversation is too long to continue safely. Please start a new conversation."
                .to_string()
        } else {
            "This conversation is too long to continue safely after compaction. Please start a new conversation."
                .to_string()
        },
    }
}

fn cancelled_outcome() -> LoopOutcome {
    LoopOutcome::Cancelled {
        message: "I stopped this request. Any action that had already started was recorded, and no new actions will be dispatched."
            .to_string(),
    }
}

async fn invoke_host<H: ToolHost>(
    host: &H,
    call: &ToolCall,
    max_result_bytes: usize,
    tool_timeout: std::time::Duration,
) -> ToolResult {
    let invocation = std::panic::AssertUnwindSafe(host.invoke(call)).catch_unwind();
    let outcome = match tokio::time::timeout(tool_timeout, invocation).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_)) => {
            return ToolResult::error(
                call,
                ToolError {
                    class: ToolErrorClass::Handler,
                    retryable: false,
                    detail: "tool handler panicked".to_string(),
                },
            )
        }
        Err(_) => {
            return ToolResult::error(
                call,
                ToolError {
                    class: ToolErrorClass::Timeout,
                    retryable: true,
                    detail: "tool handler exceeded its deadline".to_string(),
                },
            )
        }
    };
    ToolResult::bounded(call, outcome, max_result_bytes)
}

fn no_progress_outcome() -> LoopOutcome {
    LoopOutcome::Exhausted {
        reason: "no_progress".to_string(),
        message: "I wasn't able to make further progress on that request.".to_string(),
    }
}

fn response_calls(response: &ModelResponse) -> Vec<ToolCall> {
    response
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::ToolCall { call } => Some(call.clone()),
            AssistantBlock::Text { .. } => None,
        })
        .collect()
}

fn args_error(error: serde_json::Error) -> ToolError {
    ToolError {
        class: ToolErrorClass::InvalidArguments,
        retryable: true,
        detail: format!("invalid arguments: {error}"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinishArgs {
    message: String,
    status: String,
    #[serde(default)]
    resolved_effect_ids: Vec<String>,
    #[serde(default)]
    unresolved_effect_ids: Vec<String>,
}

fn parse_finish(call: &ToolCall) -> Result<LoopOutcome, ToolError> {
    let args: FinishArgs =
        serde_json::from_value(call.arguments.clone()).map_err(|error| ToolError {
            class: ToolErrorClass::CompletionRejected,
            retryable: true,
            detail: format!("invalid finish arguments: {error}"),
        })?;
    let _effect_count = args
        .resolved_effect_ids
        .len()
        .saturating_add(args.unresolved_effect_ids.len());
    if args.message.trim().is_empty() {
        return Err(ToolError {
            class: ToolErrorClass::CompletionRejected,
            retryable: true,
            detail: "finish message must not be empty".to_string(),
        });
    }
    if !matches!(
        args.status.as_str(),
        "completed" | "partial" | "blocked" | "refused"
    ) {
        return Err(ToolError {
            class: ToolErrorClass::CompletionRejected,
            retryable: true,
            detail: "finish status is not in the closed vocabulary".to_string(),
        });
    }
    if args.status == "completed" && !args.unresolved_effect_ids.is_empty() {
        return Err(ToolError {
            class: ToolErrorClass::CompletionRejected,
            retryable: true,
            detail: "completed status cannot contain unresolved effects".to_string(),
        });
    }
    Ok(LoopOutcome::Finished {
        message: args.message,
        status: args.status,
    })
}
