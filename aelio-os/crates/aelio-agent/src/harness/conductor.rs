//! Conductor + starter harness catalog (v1 control plane).
//!
//! Preinstalled programs for proving the architecture. Selection is rule-first;
//! escalate falls through to cold ProposePath only when Conductor chooses it.

use super::{HarnessSession, StackControl};

/// Starter harness ids shipped with the OS slice.
pub const CONDUCTOR_ID: &str = "conductor";
pub const QUICK_REPLY_ID: &str = "quick_reply";
pub const UNDERSTAND_INTENT_ID: &str = "understand_intent";
pub const WAIT_FOR_USER_ID: &str = "wait_for_user";
pub const MEMORY_ATTACH_ID: &str = "memory_attach";
pub const ESCALATE_ID: &str = "escalate";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterHarness {
    QuickReply,
    UnderstandIntent,
    WaitForUser,
    MemoryAttach,
    /// Explicit fall-through to cold ProposePath / tools.
    Escalate,
}

impl StarterHarness {
    pub fn id(self) -> &'static str {
        match self {
            Self::QuickReply => QUICK_REPLY_ID,
            Self::UnderstandIntent => UNDERSTAND_INTENT_ID,
            Self::WaitForUser => WAIT_FOR_USER_ID,
            Self::MemoryAttach => MEMORY_ATTACH_ID,
            Self::Escalate => ESCALATE_ID,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::QuickReply => "Answer briefly in one or two sentences from current context",
            Self::UnderstandIntent => {
                "Clarify what the user wants; suggest the next harness; may ask one question"
            }
            Self::WaitForUser => "Ask one clarifying question and wait for the next message",
            Self::MemoryAttach => "Search memory and attach a short snippet to the context page",
            Self::Escalate => "No starter harness fits; use cold procedure proposal over tools",
        }
    }
}

/// Catalog entries Conductor may choose among (excluding escalate in listings for prompts).
pub fn starter_catalog() -> &'static [(StarterHarness, &'static str)] {
    &[
        (StarterHarness::QuickReply, "Answer briefly in one or two sentences from current context"),
        (
            StarterHarness::UnderstandIntent,
            "Clarify what the user wants; suggest the next harness; may ask one question",
        ),
        (
            StarterHarness::WaitForUser,
            "Ask one clarifying question and wait for the next message",
        ),
        (
            StarterHarness::MemoryAttach,
            "Search memory and attach a short snippet to the context page",
        ),
    ]
}

/// Rule-first Conductor selection for the starter library.
pub fn select_starter_harness(utterance: &str, session: &HarnessSession) -> StarterHarness {
    let text = utterance.trim().to_lowercase();
    if text.is_empty() {
        return StarterHarness::QuickReply;
    }
    if matches!(
        crate::harness::detect_stack_control(utterance),
        StackControl::Fresh | StackControl::ExitUp
    ) {
        return StarterHarness::QuickReply;
    }
    // Explicit wait / clarify asks.
    if text.contains("wait")
        || text.contains("hold on")
        || text.contains("what do you need")
        || text.contains("ask me")
    {
        return StarterHarness::WaitForUser;
    }
    // Memory-ish.
    if text.contains("remember")
        || text.contains("recall")
        || text.contains("what did i say")
        || text.contains("from earlier")
    {
        return StarterHarness::MemoryAttach;
    }
    // Domain intents that need registered tools → cold ProposePath. Checked before the
    // short-utterance quick_reply heuristic so "show me my appointments" (4 words) is not
    // answered from context without calling list_appointments.
    const TOOL_INTENT_HINTS: &[&str] = &[
        // effectful / write
        "send otp",
        "login",
        "log in",
        "verify otp",
        "create job",
        "list jobs",
        "delete",
        "cancel",
        "pay",
        "book",
        "reschedule",
        "register",
        // read / lookup
        "show me",
        "show my",
        "show all",
        "list my",
        "list all",
        "what are my",
        "what is my",
        "what's my",
        "get my",
        "view my",
        "see my",
        "check my",
        "look up",
        "lookup",
        "retrieve",
        "provide me",
        "appointment",
        "prescription",
        "lab result",
        "billing",
        "invoice",
        "schedule",
        "doctor",
        "patient",
        "order status",
        "my orders",
        "my order",
    ];
    if TOOL_INTENT_HINTS.iter().any(|h| text.contains(h)) {
        return StarterHarness::Escalate;
    }
    // Ambiguous / deep task → understand first.
    if text.len() > 80
        || text.contains('?')
            && (text.contains("how")
                || text.contains("why")
                || text.contains("which")
                || text.contains("should"))
        || text.contains("help me")
        || text.contains("i want to")
        || text.contains("i need to")
    {
        return StarterHarness::UnderstandIntent;
    }
    // Greetings / tiny chat → quick reply.
    if text.split_whitespace().count() <= 4
        || ["hi", "hello", "hey", "thanks", "thank you", "ok", "okay", "yes", "no"]
            .iter()
            .any(|w| text == *w || text.starts_with(&format!("{w} ")))
    {
        return StarterHarness::QuickReply;
    }
    // If conductor page already has rich notes, prefer short reply for conversational
    // follow-ups — but never swallow an explicit tool intent from above.
    if session
        .pages
        .get(CONDUCTOR_ID)
        .is_some_and(|page| page.notes.len() >= 2)
        && !TOOL_INTENT_HINTS.iter().any(|h| text.contains(h))
    {
        return StarterHarness::QuickReply;
    }
    StarterHarness::Escalate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::HarnessSession;

    #[test]
    fn selects_quick_reply_for_hello() {
        assert_eq!(
            select_starter_harness("hello", &HarnessSession::default()),
            StarterHarness::QuickReply
        );
    }

    #[test]
    fn selects_escalate_for_otp() {
        assert_eq!(
            select_starter_harness("send otp to 9611266596", &HarnessSession::default()),
            StarterHarness::Escalate
        );
    }

    #[test]
    fn selects_escalate_for_short_appointment_query() {
        assert_eq!(
            select_starter_harness("show me my appointments", &HarnessSession::default()),
            StarterHarness::Escalate
        );
    }

    #[test]
    fn selects_escalate_for_appointment_query_even_with_rich_notes() {
        let mut session = HarnessSession::default();
        session.attach_note(CONDUCTOR_ID, String::from("quick_reply handled: hi"));
        session.attach_note(CONDUCTOR_ID, String::from("quick_reply handled: thanks"));
        assert_eq!(
            select_starter_harness("could you show me appointments", &session),
            StarterHarness::Escalate
        );
    }

    #[test]
    fn selects_understand_for_help_me() {
        assert_eq!(
            select_starter_harness("help me figure out what to do next", &HarnessSession::default()),
            StarterHarness::UnderstandIntent
        );
    }
}
