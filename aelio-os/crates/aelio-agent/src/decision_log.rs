//! Human-readable per-turn Decision Log.
//!
//! The runtime already emits machine traces (`TurnTraceStep`) for every hot-loop decision. This
//! module renders them as a turn-by-turn narrative you can *read* — what each stage decided and
//! *why* — so you can watch a live conversation and judge whether the machine is behaving.
//!
//! It is a pure formatter over an already-computed [`TurnResult`]: it never calls a model, never
//! mutates state. It applies its own last-mile redaction to the input, reply, and every step rather
//! than assuming all upstream producers are safe.
//!
//! Enable in-runtime emission with `AELIO_DECISION_LOG=1` (see [`enabled`]); examples call
//! [`render`] directly.

use crate::blocks::turn::{TurnResult, TurnTraceStep};
use sha2::{Digest, Sha256};

/// True when `AELIO_DECISION_LOG` is set to a truthy value (`1`/`true`/`yes`/`on`).
pub fn enabled() -> bool {
    matches!(
        std::env::var("AELIO_DECISION_LOG")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

const RULE: &str = "────────────────────────────────────────────────────────────────────";
const BAR: &str = "════════════════════════════════════════════════════════════════════";

/// Render one turn as a readable decision block.
pub fn render(
    turn_no: u64,
    user_id: &str,
    utterance: &str,
    state_before: &str,
    result: &TurnResult,
) -> String {
    let mut out = String::new();
    out.push_str(BAR);
    out.push('\n');
    out.push_str(&format!(
        "TURN {turn_no}  user={}  utterance={:?}\n",
        stable_subject(user_id),
        truncate(&redact(utterance), 120)
    ));
    out.push_str(&format!("  state(before)={state_before}\n"));
    out.push_str(RULE);
    out.push('\n');

    for step in &result.steps {
        out.push_str(&render_step(step));
    }

    out.push_str(RULE);
    out.push('\n');

    let via = format!("{:?}", result.reply.via).to_lowercase();
    out.push_str(&format!(
        "  reply ({via}): {:?}\n",
        truncate(&redact(&result.reply.text), 160)
    ));
    out.push_str(&format!(
        "  depth={:?}  tier={}  llm_calls={}  opened_loop={}  suspended={}\n",
        result.depth,
        result
            .tier
            .map(|t| format!("{t:?}"))
            .unwrap_or_else(|| "-".into()),
        result.llm_calls,
        result.opened_loop,
        result.suspended,
    ));
    out.push_str(&format!(
        "  σ={}  new_state={}  proposal={}\n",
        result.situation_hash.as_deref().unwrap_or("-"),
        result.new_state.as_deref().unwrap_or("-"),
        result.proposal_id.as_deref().unwrap_or("-"),
    ));
    out.push_str(&format!(
        "  Commit(runtime)    turn=ok state={} flow={}\n",
        result.new_state.as_deref().unwrap_or("unchanged"),
        result
            .active_flow
            .as_ref()
            .map(|flow| flow.flow_id.as_str())
            .unwrap_or("none"),
    ));
    out.push_str(BAR);
    out.push('\n');
    out
}

pub fn render_durable_commit(turn_id: &str, user_id: &str) -> String {
    format!(
        "  Persist(durable)    turn={} user={} status=completed",
        stable_subject(turn_id),
        stable_subject(user_id),
    )
}

fn render_step(step: &TurnTraceStep) -> String {
    let name = &step.name;
    let mut block = format!("  {name:<18} {}\n", redact(&step.detail));
    if let Some(why) = explain(name) {
        block.push_str(&format!("  {:<18} → {why}\n", ""));
    }
    block
}

/// Plain-English gloss for each known hot-loop stage: what it is doing and why it sits here.
fn explain(step_name: &str) -> Option<&'static str> {
    let why = match step_name {
        "Sense" => "reads env/session/state — pure, free, and never mutates (sense before classify)",
        "FlowGate" => "flow context outranks utterance semantics; a parked step resumes before anything else",
        "FlowMatch" => "authored flow triggers are checked before semantic depth, so shallow wording cannot bypass a control rail",
        "SplitClauses" => "pure pre-gate handles obvious cases; ambiguous conjunctions use one closed, bounded language-model extraction when configured",
        "MultiClauseGate" => "two+ independent requests — ask for ordering instead of silently dropping one",
        "ClassifyDepth" => "cheapest triage first; escalates to the model only when the margin is thin",
        "Understand.Depth" => "a thin pure triage margin used one closed shallow/boundary/deep extraction; invalid output falls back safely",
        "ClassifyReplyType" => "generic/input-oriented → free template; output-oriented/banter → synthesis",
        "SituationKey" => "bucketed σ (no raw ids, no personality) so warm situations actually collide and hit",
        "LookupTier" => "0 exact σ hit · 1 near hit · 2 typed composition · 3 model proposes a path",
        "Tier2Compose" => "type-directed search stitched a known path — vocabulary, not a cache miss",
        "ExecutePromoted" => "ran the promoted deterministic path — no model in the loop",
        "ProposePath" => "cold start only: the model proposes an ordered path over declared abilities",
        "TypeCheck" => "each step's postcondition must satisfy the next's precondition before anything runs",
        "BoundaryAnswer" => "answerable from state + world knowledge; one synthesis call, no tools",
        "ActivateFlow" => "hard preconditions + policy filtered first; the trigger margin only broke ties",
        "ResumeFlow" => "re-checks policy, state, and pinned tool/prompt versions — never trusts stale auth",
        "Registry.LookupTool" => "capability tag resolves to a tool — a lookup, never a model's judgement",
        "Policy.Wrap" => "policy is a structural invariant around every effect, not a composable step",
        "Bind.Residual" => "only the argument the system genuinely lacks is asked for; the rest are sourced",
        "Invoke.Call" => "the one externally effectful line; ledger brackets it, args/secrets redacted",
        "Invoke.Error" => "typed tool failure selected a deterministic recovery path; no model guessed the outcome",
        "Postcondition" => "verifies what must be true after the step — the seam that keeps flows sound",
        "TermResolve" => "term anchored to a declared attribute with polarity — antonyms don't collide",
        "TermConfirm" => "the user's yes/no resumes the exact persisted attribute plan; the original phrase is not reinterpreted",
        "TermLearn" => "only distinct behavioral confirmations accumulate; the tenant synonym promotes after the evidence gate",
        "Explore.Shadow" => "an operator-budgeted immutable runner-up ran read-only off-path; its comparison cannot change this reply",
        "Recall.Assemble" => "bounded tenant/user retrieval assembled only the relevant evidence slice",
        "Recall.Answer" => "the response is synthesized from explicit provenance claims, not model memory",
        "Recall.Error" => "retrieval failed visibly and degraded to the cold path instead of fabricating remembered facts",
        "Memory.Request" => "an explicit remember command bypasses the model and enters the durable policy-controlled memory path",
        "Memory.Store" => "only an explicit user instruction may create factual memory; policy and secret checks run first",
        "Memory.Forget" => "an exact user-scoped memory was retired with compare-and-set and is no longer recallable",
        _ => return None,
    };
    Some(why)
}

fn stable_subject(subject: &str) -> String {
    let digest = hex::encode(Sha256::digest(subject.as_bytes()));
    format!("sha256:{}", &digest[..12])
}

fn redact(text: &str) -> String {
    let mut safe = text.to_string();
    for (pattern, replacement) in [
        (
            r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b",
            "[REDACTED:email]",
        ),
        (r"\+\d(?:[\s()-]*\d){9,14}", "[REDACTED:phone]"),
        (r"\b\d{10,15}\b", "[REDACTED:phone]"),
        (r"\b\d{6}\b", "[REDACTED:otp]"),
        (
            r"(?i)bearer\s+[A-Z0-9._~+/-]+=*",
            "Bearer [REDACTED:secret]",
        ),
    ] {
        if let Ok(regex) = regex::Regex::new(pattern) {
            safe = regex.replace_all(&safe, replacement).into_owned();
        }
    }
    safe
}

fn truncate(text: &str, max: usize) -> String {
    let cleaned = text.replace('\n', " ");
    if cleaned.chars().count() <= max {
        cleaned
    } else {
        let head: String = cleaned.chars().take(max).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_inputs_even_when_upstream_trace_does_not() {
        assert_eq!(redact("otp 434543"), "otp [REDACTED:otp]");
        assert_eq!(redact("call +919876543210"), "call [REDACTED:phone]");
        assert!(!stable_subject("alice@example.com").contains("alice"));
    }
}
