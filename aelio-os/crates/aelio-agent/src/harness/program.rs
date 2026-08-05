//! Saveable harness programs (vision §10 step IR).
//!
//! A harness is data: id + description + ordered named ops. Conductor retrieves and plays
//! these programs; hardcoded `exec_*` bodies remain only as a benchmark control lane.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::conductor::{
    MEMORY_ATTACH_ID, QUICK_REPLY_ID, UNDERSTAND_INTENT_ID, WAIT_FOR_USER_ID,
};
use crate::types::{AelioError, AelioResult, ReasonCode};

/// How Conductor plays starter harness bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessPlayMode {
    /// Load `HarnessProgramV1` from the catalog library and dispatch steps (production default).
    #[default]
    Stored,
    /// Call the legacy hardcoded `exec_*` bodies (benchmark control only).
    Hardcoded,
}

/// One saveable / playable harness program.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HarnessProgramV1 {
    pub id: String,
    pub version: u32,
    pub description: String,
    pub steps: Vec<HarnessStepV1>,
}

/// Named ops from vision §10 (v0 starter subset).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HarnessStepV1 {
    /// `prompt.quick_reply` — short answer from evidence source.
    #[serde(rename = "prompt.quick_reply")]
    PromptQuickReply {
        #[serde(default)]
        evidence_from: EvidenceSource,
    },
    /// `prompt.understand_intent` — classify goal + suggest next harness.
    #[serde(rename = "prompt.understand_intent")]
    PromptUnderstandIntent,
    /// `memory.search` — retrieve claims for the query source.
    #[serde(rename = "memory.search")]
    MemorySearch {
        #[serde(default)]
        query_from: QuerySource,
    },
    /// `context.attach` — attach last search snippet (or note) to Conductor page.
    #[serde(rename = "context.attach")]
    ContextAttach {
        #[serde(default)]
        from: AttachSource,
    },
    /// Ask one clarifying question and park waiting for the user.
    #[serde(rename = "ask_and_wait")]
    AskAndWait {
        /// May contain `{clause}` placeholder.
        question_template: String,
    },
    /// Finish with the last produced reply (pop frame).
    #[serde(rename = "return.finish")]
    ReturnFinish,
    /// Return to parent (pop frame); same as finish for v0 starters.
    #[serde(rename = "return.parent")]
    ReturnParent,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    /// Tiny greetings use authored template when notes empty; else page+user synthesize.
    #[default]
    GreetingTemplateOrPageUser,
    PageAndUser,
    UserOnly,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuerySource {
    #[default]
    UserUtterance,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttachSource {
    #[default]
    LastMemorySnippet,
}

impl HarnessProgramV1 {
    pub fn validate(&self) -> AelioResult<()> {
        if self.id.trim().is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "harness program id must be non-empty",
            ));
        }
        if self.description.trim().is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "harness program description must be non-empty",
            ));
        }
        if self.steps.is_empty() {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "harness program steps must be non-empty",
            ));
        }
        let has_terminal = self.steps.iter().any(|step| {
            matches!(
                step,
                HarnessStepV1::ReturnFinish
                    | HarnessStepV1::ReturnParent
                    | HarnessStepV1::AskAndWait { .. }
            )
        });
        if !has_terminal {
            return Err(AelioError::new(
                ReasonCode::Validation,
                "harness program must end with return.finish, return.parent, or ask_and_wait",
            ));
        }
        Ok(())
    }

    /// Stable content fingerprint for retrieve-after-restart checks.
    pub fn content_hash(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        let digest = Sha256::digest(&bytes);
        hex::encode(digest)
    }
}

/// OS starter library: four playable programs mirroring the hardcoded v0 bodies.
pub fn starter_harness_library() -> IndexMap<String, HarnessProgramV1> {
    let mut library = IndexMap::new();
    for program in [
        quick_reply_program(),
        understand_intent_program(),
        wait_for_user_program(),
        memory_attach_program(),
    ] {
        program
            .validate()
            .expect("seed starter harness programs must validate");
        library.insert(program.id.clone(), program);
    }
    library
}

pub fn quick_reply_program() -> HarnessProgramV1 {
    HarnessProgramV1 {
        id: QUICK_REPLY_ID.into(),
        version: 1,
        description: "Answer briefly in one or two sentences from current context".into(),
        steps: vec![
            HarnessStepV1::PromptQuickReply {
                evidence_from: EvidenceSource::GreetingTemplateOrPageUser,
            },
            HarnessStepV1::ReturnFinish,
        ],
    }
}

pub fn understand_intent_program() -> HarnessProgramV1 {
    HarnessProgramV1 {
        id: UNDERSTAND_INTENT_ID.into(),
        version: 1,
        description:
            "Clarify what the user wants; suggest the next harness; may ask one question".into(),
        steps: vec![
            HarnessStepV1::PromptUnderstandIntent,
            HarnessStepV1::ReturnFinish,
        ],
    }
}

pub fn wait_for_user_program() -> HarnessProgramV1 {
    HarnessProgramV1 {
        id: WAIT_FOR_USER_ID.into(),
        version: 1,
        description: "Ask one clarifying question and wait for the next message".into(),
        steps: vec![HarnessStepV1::AskAndWait {
            question_template:
                "Before I continue with “{clause}”, what should I clarify first?".into(),
        }],
    }
}

pub fn memory_attach_program() -> HarnessProgramV1 {
    HarnessProgramV1 {
        id: MEMORY_ATTACH_ID.into(),
        version: 1,
        description: "Search memory and attach a short snippet to the context page".into(),
        steps: vec![
            HarnessStepV1::MemorySearch {
                query_from: QuerySource::UserUtterance,
            },
            HarnessStepV1::ContextAttach {
                from: AttachSource::LastMemorySnippet,
            },
            HarnessStepV1::PromptQuickReply {
                evidence_from: EvidenceSource::PageAndUser,
            },
            HarnessStepV1::ReturnFinish,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_library_validates_and_hashes_stably() {
        let library = starter_harness_library();
        assert_eq!(library.len(), 4);
        let qr = &library[QUICK_REPLY_ID];
        assert_eq!(qr.content_hash(), quick_reply_program().content_hash());
        let round = serde_json::from_str::<HarnessProgramV1>(
            &serde_json::to_string(qr).unwrap(),
        )
        .unwrap();
        assert_eq!(round, *qr);
    }

    #[test]
    fn rejects_empty_steps() {
        let bad = HarnessProgramV1 {
            id: "x".into(),
            version: 1,
            description: "x".into(),
            steps: vec![],
        };
        assert!(bad.validate().is_err());
    }
}
