//! Kernel-owned todo board — prosthetic attention for multi-step work.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoBoard {
    items: BTreeMap<String, TodoItem>,
}

impl TodoBoard {
    pub fn write(&mut self, items: Vec<TodoItem>) -> Result<Value, String> {
        if items.is_empty() {
            return Err("write_todos requires at least one item".into());
        }
        if items.len() > 64 {
            return Err("write_todos accepts at most 64 items".into());
        }
        let mut seen = std::collections::HashSet::new();
        for item in &items {
            if item.id.trim().is_empty() || item.id.len() > 64 {
                return Err("todo id must be 1..=64 characters".into());
            }
            if item.content.trim().is_empty() || item.content.len() > 512 {
                return Err("todo content must be 1..=512 characters".into());
            }
            if !seen.insert(item.id.clone()) {
                return Err(format!("duplicate todo id `{}`", item.id));
            }
        }
        self.items.clear();
        for item in items {
            self.items.insert(item.id.clone(), item);
        }
        Ok(self.snapshot())
    }

    pub fn update(&mut self, updates: Vec<(String, TodoStatus)>) -> Result<Value, String> {
        if updates.is_empty() {
            return Err("update_todos requires at least one update".into());
        }
        for (id, status) in updates {
            let item = self
                .items
                .get_mut(&id)
                .ok_or_else(|| format!("unknown todo id `{id}`"))?;
            item.status = status;
        }
        Ok(self.snapshot())
    }

    pub fn ensure_for_spawn(&mut self, task_id: &str, goal: &str) {
        if self.items.contains_key(task_id) {
            return;
        }
        self.items.insert(
            task_id.to_string(),
            TodoItem {
                id: task_id.to_string(),
                content: goal.chars().take(512).collect(),
                status: TodoStatus::InProgress,
            },
        );
    }

    pub fn mark(&mut self, id: &str, status: TodoStatus) {
        if let Some(item) = self.items.get_mut(id) {
            item.status = status;
        }
    }

    pub fn open_ids(&self) -> Vec<String> {
        self.items
            .values()
            .filter(|item| item.status.is_open())
            .map(|item| item.id.clone())
            .collect()
    }

    pub fn has_open(&self) -> bool {
        self.items.values().any(|item| item.status.is_open())
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "todos": self.items.values().cloned().collect::<Vec<_>>(),
            "open": self.open_ids(),
        })
    }
}
