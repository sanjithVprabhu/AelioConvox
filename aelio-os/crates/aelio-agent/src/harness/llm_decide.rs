//! LLM-backed `conductor.decide` — host wire for Path B.
//!
//! Calls [`LlmProvider`] with a closed JSON schema and maps the result to
//! [`aelio_kernel::ModelVerdict`]. On provider/parse failure the caller should
//! fall back to [`aelio_kernel::scripted_decide`].

use aelio_kernel::{ModelVerdict, KIND_QUICK_REPLY, KIND_ROUGH_CHAT, KIND_SPAWN};
use aelio_sol::SolValue;
use indexmap::IndexMap;

use crate::provider::{
    ClosedOutputSpec, ClosedOutputType, LlmProvider, LlmRequest, PromptSlotSpec, PromptSpec,
};
use crate::types::{AelioError, AelioResult, ReasonCode, Sensitivity};

pub const DECIDE_SPEC_ID: &str = "aelio.conductor.decide";
pub const DECIDE_SPEC_VERSION: &str = "1";

fn decide_prompt_spec() -> PromptSpec {
    PromptSpec {
        id: DECIDE_SPEC_ID.into(),
        version: DECIDE_SPEC_VERSION.into(),
        instruction: "You are the Aelio Conductor. Given the user utterance and the installed harness catalog, choose exactly one action. If ANY catalog harness clearly fits the user request, you MUST kind=spawn with that harness_id — do not explain via quick_reply/rough_chat instead. Prefer spawn over chat whenever a catalog summary matches. Use quick_reply only for short greetings/acks with no task. Use rough_chat for open conversation or when nothing in the catalog fits (never invent a harness_id outside the catalog). Always return all fields. For non-spawn, set harness_id/x/y to empty strings. For spawn, harness_id MUST be a catalog id; fill x/y when the harness needs numbers. Fill reply_text for rough_chat/quick_reply; empty string for spawn is fine.".into(),
        slots: vec![
            PromptSlotSpec {
                name: "utterance".into(),
                required: true,
                sensitivity: Sensitivity::None,
            },
            PromptSlotSpec {
                name: "catalog".into(),
                required: true,
                sensitivity: Sensitivity::None,
            },
        ],
        max_chars: 12_000,
        max_tokens: 512,
        truncation_order: vec!["catalog".into(), "utterance".into()],
        output: ClosedOutputSpec {
            required_fields: vec![
                "kind".into(),
                "harness_id".into(),
                "reply_text".into(),
                "confidence".into(),
                "x".into(),
                "y".into(),
            ],
            allowed_fields: vec![
                "kind".into(),
                "harness_id".into(),
                "reply_text".into(),
                "confidence".into(),
                "x".into(),
                "y".into(),
            ],
            field_types: indexmap::indexmap! {
                "kind".into() => ClosedOutputType::String,
                "harness_id".into() => ClosedOutputType::String,
                "reply_text".into() => ClosedOutputType::String,
                "confidence".into() => ClosedOutputType::String,
                "x".into() => ClosedOutputType::String,
                "y".into() => ClosedOutputType::String,
            },
        },
    }
}

fn format_catalog(catalog: &SolValue) -> String {
    let Some(items) = catalog.as_list() else {
        return "[]".into();
    };
    let mut lines = Vec::new();
    for item in items {
        let Some(m) = item.as_map() else { continue };
        let id = m
            .get("id")
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("?");
        let summary = m
            .get("summary")
            .and_then(|v| match v {
                SolValue::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        lines.push(format!("- {id}: {summary}"));
    }
    if lines.is_empty() {
        "[]".into()
    } else {
        lines.join("\n")
    }
}

fn parse_kind(raw: &str) -> AelioResult<&'static str> {
    match raw.trim().to_lowercase().as_str() {
        "spawn" => Ok(KIND_SPAWN),
        "rough_chat" | "rough-chat" | "chat" => Ok(KIND_ROUGH_CHAT),
        "quick_reply" | "quick-reply" | "reply" => Ok(KIND_QUICK_REPLY),
        other => Err(AelioError::new(
            ReasonCode::ParseError,
            format!("unknown decide kind `{other}`"),
        )),
    }
}

fn parse_opt_i64(v: Option<&str>) -> Option<i64> {
    v.and_then(|s| s.trim().parse().ok())
}

fn parse_confidence(v: Option<&str>) -> f64 {
    v.and_then(|s| s.trim().parse().ok())
        .filter(|c| (0.0..=1.0).contains(c))
        .unwrap_or(0.7)
}

/// Ask the host LLM for a Conductor decision.
pub fn decide_with_llm(
    provider: &mut dyn LlmProvider,
    utterance: &str,
    catalog: &SolValue,
) -> AelioResult<ModelVerdict> {
    let full = decide_with_llm_full(provider, utterance, catalog)?;
    Ok(full.verdict)
}

/// Full decide including reply_text for non-spawn kinds.
#[derive(Debug, Clone)]
pub struct LlmDecideOutcome {
    pub verdict: ModelVerdict,
    pub reply_text: String,
}

pub fn decide_with_llm_full(
    provider: &mut dyn LlmProvider,
    utterance: &str,
    catalog: &SolValue,
) -> AelioResult<LlmDecideOutcome> {
    let spec = decide_prompt_spec();
    let catalog_text = format_catalog(catalog);
    let prompt = spec.render(&IndexMap::from([
        ("utterance".into(), utterance.into()),
        ("catalog".into(), catalog_text),
    ]))?;
    let response = provider.complete(&LlmRequest {
        prompt,
        model: "default".into(),
        temperature: 0.0,
    })?;
    let parsed = spec.parse_output(&response.content)?;
    let kind = parse_kind(
        parsed
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    )?;
    let harness_id = parsed
        .get("harness_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let reply_text = parsed
        .get("reply_text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let confidence = parse_confidence(parsed.get("confidence").and_then(serde_json::Value::as_str));
    let x = parse_opt_i64(parsed.get("x").and_then(serde_json::Value::as_str));
    let y = parse_opt_i64(parsed.get("y").and_then(serde_json::Value::as_str));
    if kind == KIND_SPAWN && harness_id.is_none() {
        return Err(AelioError::new(
            ReasonCode::ParseError,
            "spawn requires harness_id",
        ));
    }
    Ok(LlmDecideOutcome {
        verdict: ModelVerdict {
            kind: kind.into(),
            harness_id,
            confidence,
            x,
            y,
            reply_text: if reply_text.is_empty() {
                None
            } else {
                Some(reply_text.clone())
            },
        },
        reply_text,
    })
}

/// One sim-catalog turn: LLM decide → Conductor Sol spawn/reply.
pub fn run_catalog_sim_turn_with_llm(
    provider: &mut dyn LlmProvider,
    utterance: &str,
) -> AelioResult<aelio_kernel::ConductorSolTurnResult> {
    use aelio_kernel::{run_conductor_catalog_sim_turn, sim_decide_catalog, DecideMode};
    use std::sync::Arc;

    let catalog = sim_decide_catalog();
    let verdict = decide_with_llm(provider, utterance, &catalog)?;
    let mode = DecideMode::Model(Arc::new(move |_args| Ok(verdict.clone())));
    run_conductor_catalog_sim_turn(utterance, mode).map_err(|e| {
        AelioError::new(
            ReasonCode::Internal,
            format!("conductor catalog sim failed: {e:?}"),
        )
    })
}

/// Full simulation chat through the LLM decide wire (provider supplies each turn).
pub fn run_sim_chat_with_llm(
    provider: &mut dyn LlmProvider,
) -> AelioResult<Vec<aelio_kernel::ConductorSolTurnResult>> {
    run_sim_chat_script_with_llm(provider, &aelio_kernel::sim_chat_script())
}

/// Run an arbitrary sim script through the LLM decide wire.
pub fn run_sim_chat_script_with_llm(
    provider: &mut dyn LlmProvider,
    script: &[aelio_kernel::SimChatExpect],
) -> AelioResult<Vec<aelio_kernel::ConductorSolTurnResult>> {
    let mut out = Vec::new();
    for step in script {
        out.push(run_catalog_sim_turn_with_llm(provider, step.utterance)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{LlmResponse, ScriptedLlmProvider};
    use aelio_kernel::{
        grade_sim_turn, sim_chat_script, sim_chat_script_diverse, KIND_QUICK_REPLY,
        KIND_ROUGH_CHAT, KIND_SPAWN,
    };

    fn scripted_decide_json(
        kind: &str,
        harness_id: Option<&str>,
        reply_text: &str,
    ) -> String {
        serde_json::json!({
            "kind": kind,
            "harness_id": harness_id.unwrap_or(""),
            "reply_text": reply_text,
            "confidence": "0.9",
            "x": "",
            "y": "",
        })
        .to_string()
    }

    fn responses_for_script(
        script: &[aelio_kernel::SimChatExpect],
    ) -> Vec<AelioResult<LlmResponse>> {
        script
            .iter()
            .map(|step| {
                let reply = match step.kind {
                    KIND_QUICK_REPLY => "Hey! I can help you.",
                    KIND_ROUGH_CHAT => "Tell me more about what you need.",
                    _ => "",
                };
                // For allow_kinds-only OOD rows, emit the primary kind.
                Ok(LlmResponse {
                    content: scripted_decide_json(step.kind, step.harness_id, reply),
                    provider_request_id: None,
                    input_tokens: None,
                    output_tokens: None,
                })
            })
            .collect()
    }

    #[test]
    fn llm_decide_parses_spawn() {
        let body = r#"{"kind":"spawn","harness_id":"demo.sum_ok","reply_text":"","confidence":"0.9","x":"2","y":"3"}"#;
        let mut provider = ScriptedLlmProvider::new([Ok(LlmResponse {
            content: body.into(),
            provider_request_id: None,
            input_tokens: None,
            output_tokens: None,
        })]);
        let catalog = SolValue::List(vec![SolValue::map([
            ("id", SolValue::str("demo.sum_ok")),
            ("summary", SolValue::str("sum check")),
        ])]);
        let out = decide_with_llm_full(&mut provider, "check 2 and 3", &catalog).unwrap();
        assert_eq!(out.verdict.kind, KIND_SPAWN);
        assert_eq!(out.verdict.harness_id.as_deref(), Some("demo.sum_ok"));
        assert_eq!(out.verdict.x, Some(2));
        assert_eq!(out.verdict.y, Some(3));
    }

    #[test]
    fn scripted_llm_sim_chat_hits_every_harness() {
        let script = sim_chat_script();
        let mut provider = ScriptedLlmProvider::new(responses_for_script(&script));
        let results = run_sim_chat_with_llm(&mut provider).unwrap();
        assert_eq!(results.len(), script.len());
        for (got, expect) in results.iter().zip(script.iter()) {
            let grade = grade_sim_turn(expect, got);
            assert!(
                grade.choice_ok && grade.executable_ok,
                "{} grade={:?}",
                expect.utterance,
                grade
            );
        }
    }

    #[test]
    fn scripted_llm_diverse_sim_is_executable() {
        let script = sim_chat_script_diverse();
        let mut provider = ScriptedLlmProvider::new(responses_for_script(&script));
        let results = run_sim_chat_script_with_llm(&mut provider, &script).unwrap();
        assert_eq!(results.len(), script.len());
        for (got, expect) in results.iter().zip(script.iter()) {
            let grade = grade_sim_turn(expect, got);
            assert!(
                grade.executable_ok,
                "category={} {} not executable: {:?}",
                expect.category,
                expect.utterance,
                grade.executable_error
            );
            assert!(
                grade.choice_ok,
                "category={} {} wrong choice {:?}",
                expect.category,
                expect.utterance,
                got
            );
        }
    }
}
