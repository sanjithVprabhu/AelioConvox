//! Structured reflection after tool failure or progress stall (not LLM self-grading).

use blake3;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectionEpisode {
    pub signature: String,
    pub failed_action: String,
    pub error: String,
    pub recent_context_hash: String,
    pub repair_hypothesis: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflectionLog {
    signatures: BTreeSet<String>,
    episodes: Vec<ReflectionEpisode>,
}

impl ReflectionLog {
    pub fn maybe_reflect_tool_error(
        &mut self,
        tool: &str,
        detail: &str,
        context_hash: &str,
    ) -> Option<Value> {
        let signature = failure_signature(tool, detail);
        if !self.signatures.insert(signature.clone()) {
            return None;
        }
        let episode = ReflectionEpisode {
            signature: signature.clone(),
            failed_action: tool.to_string(),
            error: detail.to_string(),
            recent_context_hash: context_hash.to_string(),
            repair_hypothesis: format!(
                "Do not repeat the identical `{tool}` call with the same arguments. Diagnose the error, adjust inputs or choose a different tool, then continue."
            ),
        };
        self.episodes.push(episode.clone());
        Some(json!({
            "kind": "reflection",
            "episode": episode,
        }))
    }

    pub fn maybe_reflect_stall(
        &mut self,
        kind: &str,
        detail: &str,
        context_hash: &str,
    ) -> Option<Value> {
        let signature = failure_signature(&format!("stall:{kind}"), detail);
        if !self.signatures.insert(signature.clone()) {
            return None;
        }
        let episode = ReflectionEpisode {
            signature,
            failed_action: format!("progress_stall:{kind}"),
            error: detail.to_string(),
            recent_context_hash: context_hash.to_string(),
            repair_hypothesis:
                "Change strategy: gather missing inputs, use a different tool, spawn a scoped sub-task, or finish partial/blocked."
                    .to_string(),
        };
        self.episodes.push(episode.clone());
        Some(json!({
            "kind": "reflection",
            "episode": episode,
        }))
    }

    pub fn episode_count(&self) -> usize {
        self.episodes.len()
    }
}

fn failure_signature(tool: &str, detail: &str) -> String {
    let bytes = format!("{tool}\n{detail}").into_bytes();
    blake3::hash(&bytes).to_hex().to_string()
}
