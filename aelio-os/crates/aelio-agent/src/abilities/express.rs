//! Express.* — surface generation.
//! Template is pure; Synthesize is the only open-output ability and is terminal.

use crate::abilities::judge::{verify_claim_refs, EvidenceClaim};
use crate::provider::{
    ClosedOutputSpec, ClosedOutputType, LlmProvider, LlmRequest, PromptSlotSpec, PromptSpec,
};
use crate::tenant::PersonalitySpec;
use crate::types::{AelioResult, ReplyType, Sensitivity, Value};
use aelio_render::{confirm_block, text_frame, validate_render_frame, RenderFrame, RenderMode};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Utterance {
    pub text: String,
    pub via: ExpressVia,
    pub template_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claim_refs: Vec<String>,
    /// Render Protocol frame (AELIO_RENDER_PROTOCOL). When absent at construction,
    /// [`Utterance::ensure_render_frame`] fills a frame from `text` / `via`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<RenderFrame>,
}

impl Utterance {
    /// Ensure a validated RenderFrame exists.
    /// Confirm utterances get `confirm@1` (+ text fallback); others get `text@1`.
    pub fn ensure_render_frame(&mut self, turn_id: &str, frame_id: &str) -> Result<(), String> {
        if self.frame.is_none() {
            let frame = match self.via {
                ExpressVia::Confirm => RenderFrame {
                    frame_type: "render".into(),
                    frame_id: frame_id.into(),
                    mode: RenderMode::Append,
                    turn_id: turn_id.into(),
                    blocks: vec![confirm_block(
                        "c0", &self.text, "Confirm", "Cancel", false, None,
                    )],
                },
                _ => text_frame(turn_id, frame_id, &self.text),
            };
            validate_render_frame(&frame, None).map_err(|e| e.to_string())?;
            self.frame = Some(frame);
        } else if let Some(frame) = &self.frame {
            validate_render_frame(frame, None).map_err(|e| e.to_string())?;
            if self.text.trim().is_empty() {
                self.text = aelio_render::flatten_to_text(frame);
            }
        }
        Ok(())
    }

    pub fn plain(text: impl Into<String>, via: ExpressVia) -> Self {
        Self {
            text: text.into(),
            via,
            template_id: None,
            claim_refs: vec![],
            frame: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpressVia {
    Template,
    Ask,
    Confirm,
    Clarify,
    Synthesize,
    Apologize,
}

pub fn synthesize_prompt_spec() -> PromptSpec {
    PromptSpec {
        id: "aelio.synthesize".into(),
        version: "1".into(),
        instruction: concat!(
            "Write the terminal user-facing response using only the supplied evidence. ",
            "Return exactly one JSON object with a text string."
        )
        .into(),
        slots: vec![
            PromptSlotSpec {
                name: "voice".into(),
                required: false,
                sensitivity: Sensitivity::None,
            },
            PromptSlotSpec {
                name: "constraints".into(),
                required: false,
                sensitivity: Sensitivity::None,
            },
            PromptSlotSpec {
                name: "evidence".into(),
                required: true,
                sensitivity: Sensitivity::Pii,
            },
        ],
        max_chars: 16_000,
        max_tokens: 4_000,
        truncation_order: vec!["constraints".into(), "evidence".into()],
        output: ClosedOutputSpec {
            required_fields: vec!["text".into()],
            allowed_fields: vec!["text".into()],
            field_types: indexmap::indexmap! {
                "text".into() => ClosedOutputType::String,
            },
        },
    }
}

pub fn synthesize_with_provider(
    evidence: &str,
    personality: Option<&PersonalitySpec>,
    constraints: &[String],
    provider: &mut dyn LlmProvider,
) -> AelioResult<Utterance> {
    let spec = synthesize_prompt_spec();
    let mut bindings = IndexMap::new();
    if let Some(personality) = personality {
        bindings.insert("voice".into(), personality.voice.register.clone());
    }
    if !constraints.is_empty() {
        bindings.insert("constraints".into(), constraints.join("\n"));
    }
    bindings.insert("evidence".into(), evidence.into());
    let response = provider.complete(&LlmRequest {
        prompt: spec.render(&bindings)?,
        model: "default".into(),
        temperature: 0.0,
    })?;
    let parsed = spec.parse_output(&response.content)?;
    let text = parsed
        .get("text")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            crate::types::AelioError::new(
                crate::types::ReasonCode::SigMismatch,
                "validated synthesis output did not contain string text",
            )
        })?
        .to_string();
    Ok(Utterance {
        text,
        via: ExpressVia::Synthesize,
        template_id: None,
        claim_refs: vec![],
        frame: None,
    })
}

/// Terminal synthesis with an auditable, closed claim-reference contract.
pub fn synthesize_grounded_with_provider(
    claims: &[EvidenceClaim],
    personality: Option<&PersonalitySpec>,
    constraints: &[String],
    provider: &mut dyn LlmProvider,
) -> AelioResult<Utterance> {
    let mut spec = synthesize_prompt_spec();
    spec.version = "2".into();
    spec.instruction = concat!(
        "Write the terminal user-facing response using only the supplied evidence claims. ",
        "Return exactly one JSON object containing text and claim_refs. Every factual statement ",
        "must be supported by the referenced claim IDs."
    )
    .into();
    spec.output.required_fields.push("claim_refs".into());
    spec.output.allowed_fields.push("claim_refs".into());
    spec.output
        .field_types
        .insert("claim_refs".into(), ClosedOutputType::StringArray);
    let evidence = serde_json::to_string(claims).map_err(|error| {
        crate::types::AelioError::new(crate::types::ReasonCode::Internal, error.to_string())
    })?;
    let mut bindings = IndexMap::new();
    if let Some(personality) = personality {
        bindings.insert("voice".into(), personality.voice.register.clone());
    }
    if !constraints.is_empty() {
        bindings.insert("constraints".into(), constraints.join("\n"));
    }
    bindings.insert("evidence".into(), evidence);
    let response = provider.complete(&LlmRequest {
        prompt: spec.render(&bindings)?,
        model: "default".into(),
        temperature: 0.0,
    })?;
    let parsed = spec.parse_output(&response.content)?;
    let text = parsed
        .get("text")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            crate::types::AelioError::new(
                crate::types::ReasonCode::SigMismatch,
                "validated grounded output did not contain string text",
            )
        })?
        .to_string();
    let refs = parsed
        .get("claim_refs")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            crate::types::AelioError::new(
                crate::types::ReasonCode::SigMismatch,
                "validated grounded output did not contain claim_refs array",
            )
        })?;
    let claim_refs: Vec<String> = refs
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                crate::types::AelioError::new(
                    crate::types::ReasonCode::SigMismatch,
                    "validated grounded output contained a non-string claim reference",
                )
            })
        })
        .collect::<AelioResult<_>>()?;
    verify_claim_refs(claims, &claim_refs)?;
    Ok(Utterance {
        text,
        via: ExpressVia::Synthesize,
        template_id: None,
        claim_refs,
        frame: None,
    })
}

pub fn template(id: &str, body: &str, bindings: &IndexMap<String, String>) -> Utterance {
    let mut text = body.to_string();
    for (k, v) in bindings {
        text = text.replace(&format!("{{{{{k}}}}}"), v);
        text = text.replace(&format!("{{{k}}}"), v);
    }
    Utterance {
        text,
        via: ExpressVia::Template,
        template_id: Some(id.into()),
        claim_refs: vec![],
        frame: None,
    }
}

pub fn ask(missing_slot: &str, hint: Option<&str>, attempt_n: u32) -> Utterance {
    let base = hint.unwrap_or("Could you provide that?").to_string();
    let text = if attempt_n > 1 {
        format!("{base} (still need: {missing_slot})")
    } else {
        base
    };
    Utterance {
        text,
        via: ExpressVia::Ask,
        template_id: None,
        claim_refs: vec![],
        frame: None,
    }
}

pub fn confirm(action: &str, args_summary: &str) -> Utterance {
    Utterance {
        text: format!("Please confirm: {action} ({args_summary}). Reply yes or no."),
        via: ExpressVia::Confirm,
        template_id: None,
        claim_refs: vec![],
        frame: None,
    }
}

pub fn clarify(candidates: &[String]) -> Utterance {
    let list = candidates.join(", ");
    Utterance {
        text: format!("Did you mean one of: {list}?"),
        via: ExpressVia::Clarify,
        template_id: None,
        claim_refs: vec![],
        frame: None,
    }
}

/// Deterministic "synthesis" for tests / warm paths that still need phrasing.
/// Production wires a real LLM sandwich here with closed output contract.
pub fn synthesize(
    evidence: &str,
    personality: Option<&PersonalitySpec>,
    constraints: &[String],
    llm_hook: Option<&mut dyn FnMut(&str) -> String>,
) -> (Utterance, bool /*used_llm*/) {
    if let Some(hook) = llm_hook {
        let mut prompt = String::new();
        if let Some(p) = personality {
            prompt.push_str(&format!("voice={:?}\n", p.voice.register));
        }
        for c in constraints {
            prompt.push_str(&format!("constraint: {c}\n"));
        }
        prompt.push_str(evidence);
        let text = hook(&prompt);
        return (
            Utterance {
                text,
                via: ExpressVia::Synthesize,
                template_id: None,
                claim_refs: vec![],
                frame: None,
            },
            true,
        );
    }
    // Deterministic fallback
    (
        Utterance {
            text: evidence.to_string(),
            via: ExpressVia::Synthesize,
            template_id: None,
            claim_refs: vec![],
            frame: None,
        },
        false,
    )
}

pub fn apologize(reason: &str, recovery: &str) -> Utterance {
    Utterance {
        text: format!("Sorry — {reason}. {recovery}"),
        via: ExpressVia::Apologize,
        template_id: None,
        claim_refs: vec![],
        frame: None,
    }
}

/// Dispatch: Generic/InputOriented → Template; OutputOriented/Banter → Synthesize.
pub fn choose_express_mode(reply: ReplyType) -> ExpressVia {
    match reply {
        ReplyType::Generic | ReplyType::InputOriented => ExpressVia::Template,
        ReplyType::OutputOriented | ReplyType::Banter => ExpressVia::Synthesize,
    }
}

pub fn greeting_template(
    personality: Option<&PersonalitySpec>,
    direction_target: Option<&str>,
    capabilities: &[String],
    returning: bool,
    dormant: bool,
) -> Utterance {
    if let Some(p) = personality {
        if let Some(t) = p.templates.get("greeting_unauth") {
            if !returning {
                let caps = capabilities.join(" or ");
                return template(
                    "greeting_unauth",
                    t,
                    &indexmap::indexmap! {
                        "capabilities".into() => caps,
                        "direction".into() => direction_target.unwrap_or("").into(),
                    },
                );
            }
        }
    }
    let caps = if capabilities.is_empty() {
        None
    } else {
        Some(capabilities.join(", "))
    };
    let text = if dormant {
        match &caps {
            Some(c) => format!("Welcome back! It's been a while — I can help with: {c}."),
            None => "Welcome back! It's been a while — how can I help?".into(),
        }
    } else if returning {
        match &caps {
            Some(c) => {
                format!("Hi again — want to pick up where you left off? I can help with: {c}.")
            }
            None => "Hi again — want to pick up where you left off?".into(),
        }
    } else {
        match &caps {
            Some(c) => format!("Hey! I can help with: {c}."),
            None => "Hey! How can I help you today?".into(),
        }
    };
    Utterance {
        text,
        via: ExpressVia::Template,
        template_id: Some("greeting".into()),
        claim_refs: vec![],
        frame: None,
    }
}

pub fn utterance_to_value(u: &Utterance) -> Value {
    Value::Map(indexmap::indexmap! {
        "text".into() => Value::str(&u.text),
        "via".into() => Value::str(format!("{:?}", u.via).to_lowercase()),
    })
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn ensure_render_frame_emits_text() {
        let mut u = apologize("timeout", "try again");
        u.ensure_render_frame("turn-1", "rf-1").unwrap();
        let frame = u.frame.as_ref().unwrap();
        assert_eq!(frame.frame_type, "render");
        assert_eq!(frame.blocks[0].kind, "text@1");
        let json = serde_json::to_value(frame).unwrap();
        assert_eq!(json["frame"], "render");
        assert_eq!(json["blocks"][0]["body"]["md"], u.text);
    }

    #[test]
    fn ensure_render_frame_emits_confirm() {
        let mut u = confirm("cancel_order", "order=42");
        u.ensure_render_frame("turn-2", "rf-2").unwrap();
        let frame = u.frame.as_ref().unwrap();
        assert_eq!(frame.blocks[0].kind, "confirm@1");
        assert!(frame.blocks[0].fallback.is_some());
    }
}
