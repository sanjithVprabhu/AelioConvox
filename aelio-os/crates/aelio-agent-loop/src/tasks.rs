//! In-memory sub-harness task board (ProcessTree-shaped, loop-local).

use crate::{EffectClass, ToolDefinition};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub const MAX_SPAWN_DEPTH: u32 = 3;
pub const MAX_FANOUT: usize = 8;
pub const MAX_SESSION_TASKS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnMode {
    Sync,
    Async,
}

impl SpawnMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "sync" => Some(Self::Sync),
            "async" => Some(Self::Async),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Exhausted,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub goal: String,
    pub tool_allowlist: Vec<String>,
    pub budget_tokens: u64,
    pub max_turns: u32,
    pub mode: SpawnMode,
    pub parallel_ok: bool,
    pub return_schema: Option<Value>,
    pub resource_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub task_id: String,
    pub parent_id: Option<String>,
    pub depth: u32,
    pub goal: String,
    pub tool_allowlist: Vec<String>,
    pub budget_tokens: u64,
    pub tokens_used: u64,
    pub max_turns: u32,
    pub mode: SpawnMode,
    pub parallel_ok: bool,
    pub resource_keys: Vec<String>,
    pub return_schema: Option<Value>,
    pub effect_classes: Vec<EffectClass>,
    pub status: TaskStatus,
    pub summary: Option<String>,
    pub data: Option<Value>,
    pub effects_performed: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    pub task_id: String,
    pub status: TaskStatus,
    pub summary: String,
    pub data: Option<Value>,
    pub effects_performed: Vec<Value>,
    pub tokens_used: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskBoard {
    pub root_id: String,
    pub depth: u32,
    pub remaining_tokens: u64,
    tasks: BTreeMap<String, TaskRecord>,
    next_seq: u64,
}

impl TaskBoard {
    pub fn new(root_id: impl Into<String>, remaining_tokens: u64, depth: u32) -> Self {
        Self {
            root_id: root_id.into(),
            depth,
            remaining_tokens,
            tasks: BTreeMap::new(),
            next_seq: 1,
        }
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.tasks
            .values()
            .filter(|task| task.status == TaskStatus::Running)
            .map(|task| task.task_id.clone())
            .collect()
    }

    pub fn any_running(&self) -> bool {
        self.tasks
            .values()
            .any(|task| task.status == TaskStatus::Running)
    }

    pub fn get(&self, task_id: &str) -> Option<&TaskRecord> {
        self.tasks.get(task_id)
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "pending_tasks": self.running_ids(),
            "tasks": self.tasks.values().cloned().collect::<Vec<_>>(),
            "remaining_tokens": self.remaining_tokens,
            "depth": self.depth,
        })
    }

    pub fn spawn(
        &mut self,
        request: SpawnRequest,
        parent_tools: &[ToolDefinition],
        kernel_names: &HashSet<&str>,
    ) -> Result<(String, SpawnMode, bool), String> {
        if self.depth >= MAX_SPAWN_DEPTH {
            return Err(format!(
                "spawn depth cap is {MAX_SPAWN_DEPTH}; deeper spawns are rejected"
            ));
        }
        if self.running_ids().len() >= MAX_FANOUT {
            return Err(format!("max {MAX_FANOUT} concurrent children per parent"));
        }
        if self.tasks.len() >= MAX_SESSION_TASKS {
            return Err(format!("max {MAX_SESSION_TASKS} tasks per session"));
        }
        if request.goal.trim().is_empty() || request.goal.len() > 4_096 {
            return Err("spawn goal must be 1..=4096 characters".into());
        }
        if request.budget_tokens == 0 {
            return Err("spawn budget_tokens must be > 0".into());
        }
        if request.budget_tokens > self.remaining_tokens {
            return Err(format!(
                "spawn budget {} exceeds remaining parent tokens {}",
                request.budget_tokens, self.remaining_tokens
            ));
        }
        if request.max_turns == 0 || request.max_turns > 120 {
            return Err("spawn max_turns must be 1..=120".into());
        }

        let authorized: HashSet<&str> = parent_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .filter(|name| !kernel_names.contains(name))
            .collect();
        let mut allowlist = Vec::new();
        let mut effect_classes = Vec::new();
        for name in &request.tool_allowlist {
            if kernel_names.contains(name.as_str()) {
                return Err(format!(
                    "child cannot receive kernel tool `{name}` in its allowlist"
                ));
            }
            if !authorized.contains(name.as_str()) {
                return Err(format!(
                    "child tool `{name}` is outside parent capability (privilege monotonicity)"
                ));
            }
            let class = parent_tools
                .iter()
                .find(|tool| tool.name == *name)
                .map(|tool| tool.effect_class.clone())
                .expect("authorized tool present");
            allowlist.push(name.clone());
            effect_classes.push(class);
        }
        if allowlist.is_empty() {
            return Err("spawn tool_allowlist must be a non-empty subset of parent tools".into());
        }

        let parallel_ok = request.parallel_ok && effects_are_parallel_safe(&effect_classes);
        let task_id = format!("task-{}", self.next_seq);
        self.next_seq = self.next_seq.saturating_add(1);
        self.remaining_tokens = self.remaining_tokens.saturating_sub(request.budget_tokens);
        self.tasks.insert(
            task_id.clone(),
            TaskRecord {
                task_id: task_id.clone(),
                parent_id: Some(self.root_id.clone()),
                depth: self.depth.saturating_add(1),
                goal: request.goal,
                tool_allowlist: allowlist,
                budget_tokens: request.budget_tokens,
                tokens_used: 0,
                max_turns: request.max_turns,
                mode: request.mode,
                parallel_ok,
                resource_keys: request.resource_keys,
                return_schema: request.return_schema,
                effect_classes,
                status: TaskStatus::Running,
                summary: None,
                data: None,
                effects_performed: Vec::new(),
            },
        );
        Ok((task_id, request.mode, parallel_ok))
    }

    pub fn complete(&mut self, result: TaskResult) -> Result<(), String> {
        if result.status == TaskStatus::Running {
            return Err(format!(
                "task `{}` executor returned a non-terminal result",
                result.task_id
            ));
        }
        let task = self
            .tasks
            .get_mut(&result.task_id)
            .ok_or_else(|| format!("unknown task `{}`", result.task_id))?;
        if task.status.is_terminal() {
            return Err(format!("task `{}` is already terminal", result.task_id));
        }
        let unused = task.budget_tokens.saturating_sub(result.tokens_used);
        self.remaining_tokens = self.remaining_tokens.saturating_add(unused);
        task.tokens_used = result.tokens_used;
        task.status = result.status;
        task.summary = Some(result.summary);
        task.data = result.data;
        task.effects_performed = result.effects_performed;
        Ok(())
    }

    pub fn cancel_subtree(&mut self, ids: &[String]) -> Value {
        let mut cancelled = Vec::new();
        let mut stack: Vec<String> = ids.to_vec();
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let children: Vec<String> = self
                .tasks
                .values()
                .filter(|task| task.parent_id.as_deref() == Some(id.as_str()))
                .map(|task| task.task_id.clone())
                .collect();
            stack.extend(children);
            if let Some(task) = self.tasks.get_mut(&id) {
                if task.status == TaskStatus::Running {
                    let unused = task.budget_tokens.saturating_sub(task.tokens_used);
                    self.remaining_tokens = self.remaining_tokens.saturating_add(unused);
                    task.status = TaskStatus::Cancelled;
                    task.summary = Some("cancelled by parent".into());
                    cancelled.push(id);
                }
            }
        }
        json!({ "cancelled": cancelled, "pending_tasks": self.running_ids() })
    }

    pub fn check(&self) -> Value {
        self.snapshot()
    }

    pub fn take_running(&self, ids: Option<&[String]>) -> Vec<TaskRecord> {
        self.tasks
            .values()
            .filter(|task| task.status == TaskStatus::Running)
            .filter(|task| {
                ids.map(|wanted| wanted.contains(&task.task_id))
                    .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    /// Partition running tasks into parallel-safe waves vs forced-serial remainder.
    pub fn schedule_wave(tasks: &[TaskRecord]) -> (Vec<TaskRecord>, Vec<TaskRecord>) {
        let mut parallel = Vec::new();
        let mut serial = Vec::new();
        let mut used_resources = BTreeSet::new();
        let mut saw_write = false;
        for task in tasks {
            let has_write = task
                .effect_classes
                .iter()
                .any(|class| class != &EffectClass::Read);
            let resource_conflict = task
                .resource_keys
                .iter()
                .any(|key| used_resources.contains(key));
            if task.parallel_ok
                && !has_write
                && !saw_write
                && !resource_conflict
                && effects_are_parallel_safe(&task.effect_classes)
            {
                for key in &task.resource_keys {
                    used_resources.insert(key.clone());
                }
                parallel.push(task.clone());
            } else {
                if has_write {
                    saw_write = true;
                }
                for key in &task.resource_keys {
                    used_resources.insert(key.clone());
                }
                serial.push(task.clone());
            }
        }
        (parallel, serial)
    }
}

pub fn effects_are_parallel_safe(classes: &[EffectClass]) -> bool {
    classes.iter().all(|class| class == &EffectClass::Read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            version: "1".to_string(),
            description: format!("Tenant capability for {name} operations."),
            input_schema: json!({"type":"object"}),
            effect_class: EffectClass::Read,
        }
    }

    #[test]
    fn spawned_task_retains_return_schema_for_executor_validation() {
        let mut board = TaskBoard::new("root", 1_000, 0);
        let schema = json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"],
            "additionalProperties": false
        });
        let (task_id, _, _) = board
            .spawn(
                SpawnRequest {
                    goal: "Find one answer".to_string(),
                    tool_allowlist: vec!["lookup".to_string()],
                    budget_tokens: 100,
                    max_turns: 3,
                    mode: SpawnMode::Sync,
                    parallel_ok: true,
                    return_schema: Some(schema.clone()),
                    resource_keys: Vec::new(),
                },
                &[tool("lookup")],
                &HashSet::new(),
            )
            .expect("spawn");
        assert_eq!(
            board
                .get(&task_id)
                .and_then(|task| task.return_schema.as_ref()),
            Some(&schema)
        );
    }

    #[test]
    fn task_board_rejects_executor_false_running_completion() {
        let mut board = TaskBoard::new("root", 1_000, 0);
        let (task_id, _, _) = board
            .spawn(
                SpawnRequest {
                    goal: "Find one answer".to_string(),
                    tool_allowlist: vec!["lookup".to_string()],
                    budget_tokens: 100,
                    max_turns: 3,
                    mode: SpawnMode::Sync,
                    parallel_ok: true,
                    return_schema: None,
                    resource_keys: Vec::new(),
                },
                &[tool("lookup")],
                &HashSet::new(),
            )
            .expect("spawn");
        let error = board
            .complete(TaskResult {
                task_id,
                status: TaskStatus::Running,
                summary: "not actually complete".to_string(),
                data: None,
                effects_performed: Vec::new(),
                tokens_used: 0,
            })
            .expect_err("running is not terminal");
        assert!(error.contains("non-terminal"));
    }
}
