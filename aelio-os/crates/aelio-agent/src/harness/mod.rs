//! Session harness stack + durable context pages (Conductor architecture).
//!
//! Product rules: `docs/architecture/HARNESS_CONDUCTOR_VISION.md`.
//! Control plane: push/pop/fresh, waiting-child ownership, context pages.
//! Playable bodies: saveable `HarnessProgramV1` programs (see `program` module).

pub mod conductor;
pub mod llm_decide;
pub mod program;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub use conductor::{
    select_starter_harness, starter_catalog, StarterHarness, CONDUCTOR_ID, ESCALATE_ID,
    MEMORY_ATTACH_ID, QUICK_REPLY_ID, UNDERSTAND_INTENT_ID, WAIT_FOR_USER_ID,
};
pub use program::{
    memory_attach_program, quick_reply_program, starter_harness_library, understand_intent_program,
    wait_for_user_program, AttachSource, EvidenceSource, HarnessPlayMode, HarnessProgramV1,
    HarnessStepV1, QuerySource,
};

/// What the next user utterance means for the harness stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackControl {
    /// Keep the current waiting child (or continue normally).
    Stay,
    /// Pop one layer only; ceiling is Conductor.
    ExitUp,
    /// Clear the stack and restart at Conductor.
    Fresh,
}

/// One frame on the session harness stack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HarnessFrame {
    pub harness_id: String,
    /// True while this frame owns the next user utterance (Park / wait).
    pub waiting: bool,
    /// Optional runtime subject key used by adaptive artifact Park/resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_key: Option<String>,
}

/// Durable working memory for one harness in a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ContextPage {
    pub harness_id: String,
    /// Free-form facts / snippets attached by ops (search, shorten, tools…).
    #[serde(default)]
    pub notes: Vec<String>,
    /// Structured slots collected inside this harness.
    #[serde(default)]
    pub slots: IndexMap<String, serde_json::Value>,
}

/// Per-user harness control state (persisted beside lifecycle state).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HarnessSession {
    #[serde(default)]
    pub stack: Vec<HarnessFrame>,
    /// Keyed by harness_id.
    #[serde(default)]
    pub pages: IndexMap<String, ContextPage>,
}

impl HarnessSession {
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    pub fn top(&self) -> Option<&HarnessFrame> {
        self.stack.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut HarnessFrame> {
        self.stack.last_mut()
    }

    pub fn waiting_child(&self) -> Option<&HarnessFrame> {
        self.stack.last().filter(|frame| frame.waiting)
    }

    pub fn push(&mut self, harness_id: impl Into<String>, waiting: bool) {
        let harness_id = harness_id.into();
        self.ensure_page(&harness_id);
        self.stack.push(HarnessFrame {
            harness_id,
            waiting,
            subject_key: None,
        });
    }

    /// Pop one layer. Returns the popped frame, if any.
    pub fn exit_up(&mut self) -> Option<HarnessFrame> {
        self.stack.pop()
    }

    /// Clear stack (pages retained for diagnostics unless `clear_pages`).
    pub fn fresh(&mut self, clear_pages: bool) {
        self.stack.clear();
        if clear_pages {
            self.pages.clear();
        }
    }

    pub fn ensure_page(&mut self, harness_id: &str) -> &mut ContextPage {
        if !self.pages.contains_key(harness_id) {
            self.pages.insert(
                harness_id.into(),
                ContextPage {
                    harness_id: harness_id.into(),
                    notes: Vec::new(),
                    slots: IndexMap::new(),
                },
            );
        }
        self.pages.get_mut(harness_id).expect("page just inserted")
    }

    pub fn attach_note(&mut self, harness_id: &str, note: impl Into<String>) {
        self.ensure_page(harness_id).notes.push(note.into());
    }
}

/// Rule-based stack-control detector (named op `intent.detect_stack_control` v0).
/// Prefer Fresh phrases over ExitUp when both could match.
pub fn detect_stack_control(utterance: &str) -> StackControl {
    let text = utterance.trim().to_lowercase();
    if text.is_empty() {
        return StackControl::Stay;
    }
    const FRESH: &[&str] = &[
        "start over",
        "start again",
        "start fresh",
        "forget everything",
        "forget all this",
        "new topic",
        "different topic",
        "cancel everything",
        "back to start",
        "reset",
    ];
    for phrase in FRESH {
        if text.contains(phrase) {
            return StackControl::Fresh;
        }
    }
    const EXIT_UP: &[&str] = &[
        "go back",
        "go up",
        "one level up",
        "exit this",
        "exit harness",
        "never mind",
        "nevermind",
        "leave this",
        "previous step",
        "back please",
    ];
    for phrase in EXIT_UP {
        if text.contains(phrase) {
            return StackControl::ExitUp;
        }
    }
    // Lone "cancel" / "exit" / "back" when short.
    let compact = text.trim_matches(|c: char| !c.is_alphanumeric() && !c.is_whitespace());
    if matches!(compact, "cancel" | "exit" | "back" | "stop" | "up") {
        return StackControl::ExitUp;
    }
    StackControl::Stay
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_push_exit_up_fresh() {
        let mut session = HarnessSession::default();
        session.push("conductor", false);
        session.push("wait_for_user", true);
        assert!(session.waiting_child().is_some());
        assert_eq!(session.waiting_child().unwrap().harness_id, "wait_for_user");
        let popped = session.exit_up().unwrap();
        assert_eq!(popped.harness_id, "wait_for_user");
        assert_eq!(session.top().unwrap().harness_id, "conductor");
        session.fresh(true);
        assert!(session.is_empty());
        assert!(session.pages.is_empty());
    }

    #[test]
    fn detect_fresh_and_exit_up() {
        assert_eq!(detect_stack_control("please start over"), StackControl::Fresh);
        assert_eq!(detect_stack_control("go back"), StackControl::ExitUp);
        assert_eq!(detect_stack_control("send otp to 9611"), StackControl::Stay);
        assert_eq!(detect_stack_control("cancel"), StackControl::ExitUp);
    }

    #[test]
    fn context_page_notes() {
        let mut session = HarnessSession::default();
        session.attach_note("conductor", "user prefers short answers");
        assert_eq!(session.pages["conductor"].notes.len(), 1);
    }
}
