//! Nested sub-harness execution hook (production or test-provided).

use crate::tasks::{TaskRecord, TaskResult, TaskStatus};
use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ChildExecutionRequest {
    pub task: TaskRecord,
}

#[async_trait]
pub trait ChildExecutor: Send + Sync {
    async fn execute(&self, request: ChildExecutionRequest) -> TaskResult;
}

/// Deterministic stub used when no executor is wired — children fail closed.
pub struct RejectingChildExecutor;

#[async_trait]
impl ChildExecutor for RejectingChildExecutor {
    async fn execute(&self, request: ChildExecutionRequest) -> TaskResult {
        TaskResult {
            task_id: request.task.task_id,
            status: TaskStatus::Failed,
            summary: "no child executor is configured for sub-harnesses".to_string(),
            data: None,
            effects_performed: Vec::new(),
            tokens_used: 0,
        }
    }
}

/// Scripted executor for unit tests.
pub struct ScriptedChildExecutor {
    pub results: std::sync::Mutex<std::collections::HashMap<String, TaskResult>>,
    pub default_summary: String,
}

impl ScriptedChildExecutor {
    pub fn new() -> Self {
        Self {
            results: std::sync::Mutex::new(std::collections::HashMap::new()),
            default_summary: "child completed".to_string(),
        }
    }

    pub fn with_result(self, goal_substring: impl Into<String>, result: TaskResult) -> Self {
        self.results
            .lock()
            .expect("script lock")
            .insert(goal_substring.into(), result);
        self
    }
}

impl Default for ScriptedChildExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChildExecutor for ScriptedChildExecutor {
    async fn execute(&self, request: ChildExecutionRequest) -> TaskResult {
        let goal = request.task.goal.clone();
        if let Some(result) = self
            .results
            .lock()
            .expect("script lock")
            .iter()
            .find(|(needle, _)| goal.contains(needle.as_str()))
            .map(|(_, result)| result.clone())
        {
            return TaskResult {
                task_id: request.task.task_id,
                status: result.status,
                summary: result.summary,
                data: result.data,
                effects_performed: result.effects_performed,
                tokens_used: result.tokens_used.min(request.task.budget_tokens),
            };
        }
        TaskResult {
            task_id: request.task.task_id,
            status: TaskStatus::Completed,
            summary: self.default_summary.clone(),
            data: Some(Value::Object(Default::default())),
            effects_performed: Vec::new(),
            tokens_used: 1.min(request.task.budget_tokens),
        }
    }
}
