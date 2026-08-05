//! Phase 4.5 — deterministic route cutover helpers.
//!
//! Only routes that are **safe, pure, and shadow-proven** are eligible for
//! authoritative OS cutover. Today: short greetings / thanks that map to
//! `quick_reply` without tools or model.

use crate::harness_syscalls::{
    decide_deterministic, run_conductor_root_deterministic, DeterministicConductorDecision,
};
use crate::error::{ErrV1, ReasonCode};
use aelio_sol::SolValue;

/// True when the utterance is a pure greeting/thanks/farewell safe for Conductor cutover.
pub fn is_greeting_cutover_utterance(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return true;
    }
    // Exact tokens only — do not cut over "hello please send otp".
    matches!(
        t.as_str(),
        "hi" | "hello" | "hey"
            | "thanks" | "thank you" | "thx" | "ty"
            | "ok" | "okay" | "k" | "kk"
            | "yes" | "yep" | "yeah" | "no" | "nope"
            | "bye" | "goodbye" | "good bye" | "see you" | "cya"
            | "good morning" | "good afternoon" | "good evening" | "good night"
            | "howdy" | "yo"
    )
}

/// Whether `/v1/turns` should serve `conductor.root` instead of the agent spine.
pub fn should_cutover_greeting(utterance: &str) -> bool {
    if !is_greeting_cutover_utterance(utterance) {
        return false;
    }
    matches!(
        decide_deterministic("user.message", utterance),
        DeterministicConductorDecision::QuickReply
            | DeterministicConductorDecision::QuickReplyGreeting
    )
}

/// Alias used by API / docs for the broader Phase 4.5 cutover set.
pub fn should_cutover_deterministic(utterance: &str) -> bool {
    should_cutover_greeting(utterance)
}

#[derive(Debug, Clone)]
pub struct CutoverGreetingResult {
    pub decision: DeterministicConductorDecision,
    pub reply_text: String,
    pub bag_hash: String,
    pub route: &'static str,
}

/// Run deterministic conductor.root for a greeting cutover.
pub fn run_greeting_cutover(utterance: &str) -> Result<CutoverGreetingResult, ErrV1> {
    if !should_cutover_greeting(utterance) {
        return Err(ErrV1::new(
            ReasonCode::Policy,
            "cutover",
            "utterance not eligible for greeting cutover",
        ));
    }
    let (decision, bag, bag_hash) =
        run_conductor_root_deterministic(utterance, "user.message")?;
    let reply_text = bag
        .as_map()
        .and_then(|m| m.get("reply"))
        .and_then(|r| r.as_map())
        .and_then(|m| m.get("text"))
        .and_then(|v| match v {
            SolValue::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "Hey! I can help you.".into());
    Ok(CutoverGreetingResult {
        decision,
        reply_text,
        bag_hash,
        route: "quick_reply",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greets_are_cutover_eligible() {
        assert!(should_cutover_greeting("hello"));
        assert!(should_cutover_greeting("Hi"));
        assert!(should_cutover_greeting("thanks"));
        assert!(should_cutover_greeting("bye"));
        assert!(should_cutover_greeting("yes"));
        assert!(!should_cutover_greeting("hello please send otp"));
        assert!(!should_cutover_greeting("help me hire someone"));
    }

    #[test]
    fn greeting_cutover_runs_sol() {
        let r = run_greeting_cutover("hello").unwrap();
        assert_eq!(r.route, "quick_reply");
        assert!(!r.reply_text.is_empty());
        assert!(!r.bag_hash.is_empty());
    }
}
