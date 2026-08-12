//! Durable orchestration bag carried across park/resume for the agent loop.

use crate::program::ProgramRegistry;
use crate::reflect::ReflectionLog;
use crate::tasks::TaskBoard;
use crate::todos::TodoBoard;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrchestrationState {
    pub todos: TodoBoard,
    pub tasks: TaskBoard,
    pub programs: ProgramRegistry,
    pub reflections: ReflectionLog,
}

impl OrchestrationState {
    pub fn new(root_id: impl Into<String>, remaining_tokens: u64, depth: u32) -> Self {
        Self {
            todos: TodoBoard::default(),
            tasks: TaskBoard::new(root_id, remaining_tokens, depth),
            programs: ProgramRegistry::default(),
            reflections: ReflectionLog::default(),
        }
    }
}

impl Default for OrchestrationState {
    fn default() -> Self {
        Self::new("root", 200_000, 0)
    }
}
