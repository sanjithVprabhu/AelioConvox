use crate::{AgentMessage, ModelRequest, ModelResponse, ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteReason {
    Compaction,
    SecretScrub,
    ProgressConstraint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRewrite {
    pub reason: RewriteReason,
    pub before_hash: String,
    pub after_hash: String,
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("context serialization failed: {0}")]
    Serialization(String),
}

#[derive(Debug, Clone)]
pub struct Context {
    system: String,
    bootstrap: String,
    tools: Vec<ToolDefinition>,
    messages: Vec<AgentMessage>,
    rewrites: Vec<ContextRewrite>,
}

impl Context {
    pub fn new(
        system: impl Into<String>,
        bootstrap: impl Into<String>,
        mut kernel_tools: Vec<ToolDefinition>,
        mut tenant_tools: Vec<ToolDefinition>,
    ) -> Self {
        tenant_tools.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
        kernel_tools.append(&mut tenant_tools);
        Self {
            system: system.into(),
            bootstrap: bootstrap.into(),
            tools: kernel_tools,
            messages: Vec::new(),
            rewrites: Vec::new(),
        }
    }

    pub fn append_user(&mut self, text: impl Into<String>) {
        self.messages.push(AgentMessage::User { text: text.into() });
    }

    pub fn append_assistant(&mut self, response: ModelResponse) {
        self.messages.push(AgentMessage::Assistant { response });
    }

    pub fn append_tool_result(&mut self, result: ToolResult) {
        self.messages.push(AgentMessage::ToolResult { result });
    }

    pub fn append_notice(&mut self, code: impl Into<String>, detail: impl Into<String>) {
        self.messages.push(AgentMessage::KernelNotice {
            code: code.into(),
            detail: detail.into(),
        });
    }

    pub fn extend_messages(&mut self, messages: impl IntoIterator<Item = AgentMessage>) {
        self.messages.extend(messages);
    }

    /// Bound long-running transcripts at a user-turn boundary. Immutable prompt segments are
    /// untouched, retained messages remain byte-identical and ordered, and the only replacement
    /// is an explicit kernel rewrite notice carrying the removed prefix hash.
    pub fn compact(&mut self, max_messages: usize) -> Result<Option<ContextRewrite>, ContextError> {
        if max_messages < 2 || self.messages.len() <= max_messages {
            return Ok(None);
        }
        let before_hash = messages_hash(&self.messages)?;
        let target = self
            .messages
            .len()
            .saturating_sub(max_messages.saturating_sub(1));
        let start = self.messages[target..]
            .iter()
            .position(|message| matches!(message, AgentMessage::User { .. }))
            .map(|offset| target + offset)
            .or_else(|| {
                self.messages
                    .iter()
                    .rposition(|message| matches!(message, AgentMessage::User { .. }))
            })
            .unwrap_or(target);
        let retained = self.messages.split_off(start);
        self.messages = vec![AgentMessage::KernelNotice {
            code: "context_compacted".to_string(),
            detail: format!("removed_prefix_hash={before_hash}; removed_messages={start}"),
        }];
        self.messages.extend(retained);
        let after_hash = messages_hash(&self.messages)?;
        let rewrite = ContextRewrite {
            reason: RewriteReason::Compaction,
            before_hash,
            after_hash,
        };
        self.rewrites.push(rewrite.clone());
        Ok(Some(rewrite))
    }

    pub fn request(&self) -> ModelRequest {
        ModelRequest {
            system: self.system.clone(),
            bootstrap: self.bootstrap.clone(),
            messages: self.messages.clone(),
            tools: self.tools.clone(),
        }
    }

    pub fn immutable_segments_hash(&self) -> Result<String, ContextError> {
        let bytes = serde_json::to_vec(&(&self.tools, &self.system, &self.bootstrap))
            .map_err(|error| ContextError::Serialization(error.to_string()))?;
        Ok(blake3::hash(&bytes).to_hex().to_string())
    }

    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }

    pub fn rewrites(&self) -> &[ContextRewrite] {
        &self.rewrites
    }
}

fn messages_hash(messages: &[AgentMessage]) -> Result<String, ContextError> {
    let bytes = serde_json::to_vec(messages)
        .map_err(|error| ContextError::Serialization(error.to_string()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_preserves_immutable_prefix_and_retains_a_complete_user_turn() {
        let mut context = Context::new("system", "bootstrap", Vec::new(), Vec::new());
        for index in 0..8 {
            context.append_user(format!("user-{index}"));
            context.append_notice("step", format!("notice-{index}"));
        }
        let immutable_before = context.immutable_segments_hash().unwrap();
        let rewrite = context.compact(5).unwrap().expect("rewrite");
        assert_eq!(rewrite.reason, RewriteReason::Compaction);
        assert_eq!(context.immutable_segments_hash().unwrap(), immutable_before);
        assert!(matches!(
            context.messages().first(),
            Some(AgentMessage::KernelNotice { code, .. }) if code == "context_compacted"
        ));
        assert!(matches!(
            context.messages().get(1),
            Some(AgentMessage::User { .. })
        ));
        assert_eq!(context.rewrites(), &[rewrite]);
    }

    #[test]
    fn compaction_is_a_noop_below_the_limit() {
        let mut context = Context::new("system", "bootstrap", Vec::new(), Vec::new());
        context.append_user("hello");
        assert!(context.compact(5).unwrap().is_none());
        assert!(context.rewrites().is_empty());
    }
}
