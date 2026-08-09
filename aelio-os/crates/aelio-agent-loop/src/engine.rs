use crate::{
    AssistantBlock, Budget, BudgetLimits, Context, EffectClass, GateDecision, LoopError, LoopEvent,
    LoopOutcome, ModelRequest, ModelResponse, ProgressMonitor, ToolCall, ToolError, ToolErrorClass,
    ToolResult, TurnFingerprint,
};
use async_trait::async_trait;
use futures_util::future::join_all;
use futures_util::FutureExt;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
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
}

impl<M, H, G> AgentLoop<M, H, G>
where
    M: Model,
    H: ToolHost,
    G: EffectGate,
{
    pub fn new(model: M, host: H, gate: G, context: Context, limits: BudgetLimits) -> Self {
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
        }
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
                    Ok(outcome) if completion_gate_accepts(&self.context, &outcome) => {
                        return Ok((outcome, self));
                    }
                    Ok(_) => {
                        self.context.append_tool_result(ToolResult::error(
                            finish_calls[0],
                            ToolError {
                                class: ToolErrorClass::CompletionRejected,
                                retryable: true,
                                detail: "completed status is inconsistent with the latest failed tool result; acknowledge the unresolved action".to_string(),
                            },
                        ));
                        continue;
                    }
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

            // Required-argument validation is a kernel boundary, not a host-side accident. A
            // missing field parks the entire response before policy, rate reservation, or SDK
            // dispatch. The next user turn lets the model reconstruct a complete invocation.
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

            // Authorize the entire model response before dispatching any call. This prevents a
            // mixed batch from partially executing before a later call asks for confirmation or
            // fails policy.
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

            if let Err(error) = self.host.preflight(&calls).await {
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

            // A model response containing only reads may run concurrently. Any response with a
            // write is kept wholly sequential so model-declared order remains the effect order.
            let results = if prepared
                .iter()
                .all(|(_, class)| class == &EffectClass::Read)
            {
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
                            invoke_one(
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
                        results.push(
                            invoke_one(
                                &self.host,
                                call,
                                self.limits.max_result_bytes,
                                self.limits.tool_timeout,
                            )
                            .await,
                        );
                    }
                }
                results
            };

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
                }
                self.events.push(LoopEvent::ToolCompleted {
                    call_id: call.id.clone(),
                    ok: result.error.is_none(),
                });
                self.context.append_tool_result(result);
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

    fn record_progress_notice(&mut self, notice: &crate::ProgressNotice) {
        self.events.push(LoopEvent::ProgressNotice {
            kind: format!("{:?}", notice.kind),
        });
        self.context
            .append_notice("progress_stall", notice.detail.clone());
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

async fn invoke_one<H: ToolHost>(
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

fn completion_gate_accepts(context: &Context, outcome: &LoopOutcome) -> bool {
    if !matches!(outcome, LoopOutcome::Finished { status, .. } if status == "completed") {
        return true;
    }
    !context
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            crate::AgentMessage::ToolResult { result } => Some(result.error.is_some()),
            _ => None,
        })
        .unwrap_or(false)
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
