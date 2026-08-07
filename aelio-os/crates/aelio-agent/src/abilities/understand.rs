//! Understand.* — utterance → structure.
//! Cheap substrate first; escalate on thin Judge.Confidence margin.

use crate::abilities::learn::cosine_similarity;
use crate::embedding::Embedder;
use crate::ops::pure::skip_split_clauses;
use crate::provider::{
    ClosedOutputSpec, ClosedOutputType, LlmProvider, LlmRequest, PromptSlotSpec, PromptSpec,
};
use crate::types::{AelioError, AelioResult, ReasonCode, Sensitivity};
use crate::types::{Depth, ReplyType, Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthClass {
    pub depth: Depth,
    pub margin: f64,
    pub substrate: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyClass {
    pub reply_type: ReplyType,
    pub margin: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairSignal {
    Normal,
    Repair,
    Frustration,
    Abandon,
}

/// Pure pre-gate then optional clause list. When skip is true, returns single clause free.
pub fn split_clauses(utterance: &str) -> (bool /*skipped_llm*/, Vec<String>) {
    if skip_split_clauses(utterance, 3, 24) {
        return (true, vec![utterance.trim().to_string()]);
    }
    // Deterministic fallback splitter for multi-clause without LLM in tests/MVP.
    let lower = utterance.to_lowercase();
    let markers = [" and ", " but ", " then ", " also ", ";"];
    for m in markers {
        if lower.contains(m) {
            let parts: Vec<String> = utterance
                .split(';')
                .flat_map(|seg| {
                    // crude split on " and "
                    let mut acc = vec![seg.to_string()];
                    for marker in [" and ", " but ", " then ", " also "] {
                        acc = acc
                            .into_iter()
                            .flat_map(|s| {
                                s.split(marker)
                                    .map(|x| x.trim().to_string())
                                    .filter(|x| !x.is_empty())
                                    .collect::<Vec<_>>()
                            })
                            .collect();
                    }
                    acc
                })
                .filter(|s| !s.is_empty())
                .collect();
            if parts.len() > 1 {
                return (true, parts); // used pure path, not LLM
            }
        }
    }
    (true, vec![utterance.trim().to_string()])
}

pub fn split_clauses_prompt_spec() -> PromptSpec {
    PromptSpec {
        id: "aelio.split_clauses".into(),
        version: "1".into(),
        instruction: concat!(
            "Split the utterance into independent requested actions, preserving order and wording. ",
            "A noun conjunction inside one request is one clause. Return one to four non-empty ",
            "clauses in exactly one JSON object."
        )
        .into(),
        slots: vec![PromptSlotSpec {
            name: "utterance".into(),
            required: true,
            sensitivity: Sensitivity::Pii,
        }],
        max_chars: 8_192,
        max_tokens: 2_048,
        truncation_order: vec!["utterance".into()],
        output: ClosedOutputSpec {
            required_fields: vec!["clauses".into()],
            allowed_fields: vec!["clauses".into()],
            field_types: indexmap::indexmap! {
                "clauses".into() => ClosedOutputType::StringArray,
            },
        },
    }
}

/// Use language intelligence only for the ambiguous conjunction case selected by the pure
/// pre-gate. Output is closed, bounded, and validated before it can influence control flow.
pub fn split_clauses_with_provider(
    utterance: &str,
    provider: &mut dyn LlmProvider,
) -> AelioResult<(Vec<String>, String)> {
    let spec = split_clauses_prompt_spec();
    let prompt_hash = spec.canonical_hash()?;
    let response = provider.complete(&LlmRequest {
        prompt: spec.render(&indexmap::indexmap! {
            "utterance".into() => utterance.into(),
        })?,
        model: "default".into(),
        temperature: 0.0,
    })?;
    let parsed = spec.parse_output(&response.content)?;
    let clauses = parsed
        .get("clauses")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| AelioError::new(ReasonCode::ParseError, "clauses must be an array"))?
        .iter()
        .map(|clause| {
            clause
                .as_str()
                .map(str::trim)
                .filter(|clause| !clause.is_empty() && clause.chars().count() <= 8_192)
                .map(str::to_owned)
                .ok_or_else(|| {
                    AelioError::new(
                        ReasonCode::ParseError,
                        "clauses must contain bounded non-empty strings",
                    )
                })
        })
        .collect::<AelioResult<Vec<_>>>()?;
    if clauses.is_empty() || clauses.len() > 4 {
        return Err(AelioError::new(
            ReasonCode::BudgetExceeded,
            "clause count must be between one and four",
        ));
    }
    Ok((clauses, prompt_hash))
}

/// Semantic-first depth classification with deterministic heuristics (no embedding store required).
pub fn classify_depth(utterance: &str) -> DepthClass {
    let t = utterance.trim();
    let words: Vec<&str> = t.split_whitespace().collect();
    let lower = t.to_lowercase();

    // Shallow greetings / thanks / capability questions
    if is_exact_greeting(utterance) {
        return DepthClass {
            depth: Depth::Shallow,
            margin: 0.71,
            substrate: "semantic".into(),
        };
    }
    if words.len() <= 2 && !lower.contains('?') {
        return DepthClass {
            depth: Depth::Shallow,
            margin: 0.4,
            substrate: "length_fallback".into(),
        };
    }

    // Boundary: answerable from state, no tools
    let boundary_hints = [
        "what time",
        "what's my name",
        "whats my name",
        "who am i",
        "how long",
        "what can you do",
        "help",
    ];
    if boundary_hints.iter().any(|h| lower.contains(h)) {
        return DepthClass {
            depth: Depth::Boundary,
            margin: 0.62,
            substrate: "semantic".into(),
        };
    }

    // Deep: tools / multi-step. Only *generic* action/structure cues live here — never a specific
    // vertical's nouns (no "invoice"/"client"/"otp"/"avaricious"). Domain-specific depth comes from
    // the declaration-driven path: `derive_intent` matches the utterance against the tenant's own
    // declared capability tags, so a novel vertical is deep because it names a reachable capability,
    // not because the engine was pre-taught that vertical's words.
    let deep_hints = [
        "login", "sign in", "cancel", "pay", "send", "verify", "book", "schedule", "update",
        "create", "delete", "refund", "most", "least", "list", "show me", "give me", "find",
        "search",
    ];
    if deep_hints.iter().any(|h| lower.contains(h)) || words.len() > 6 {
        return DepthClass {
            depth: Depth::Deep,
            margin: 0.68,
            substrate: "semantic".into(),
        };
    }

    DepthClass {
        depth: Depth::Boundary,
        margin: 0.45, // thin → escalate in production
        substrate: "semantic".into(),
    }
}

pub fn is_exact_greeting(utterance: &str) -> bool {
    matches!(
        utterance.trim().to_ascii_lowercase().as_str(),
        "hi" | "hello"
            | "hey"
            | "thanks"
            | "thank you"
            | "ok"
            | "okay"
            | "bye"
            | "good morning"
            | "good evening"
            | "yo"
            | "sup"
    )
}

pub fn classify_depth_prompt_spec() -> PromptSpec {
    PromptSpec {
        id: "aelio.classify_depth".into(),
        version: "1".into(),
        instruction: concat!(
            "Classify the utterance as shallow (social acknowledgement only), boundary ",
            "(answerable from current state or recalled knowledge without a tool), or deep ",
            "(requests an action, lookup, or multi-step operation). Return exactly one JSON object."
        )
        .into(),
        slots: vec![PromptSlotSpec {
            name: "utterance".into(),
            required: true,
            sensitivity: Sensitivity::Pii,
        }],
        max_chars: 8_192,
        max_tokens: 2_048,
        truncation_order: vec!["utterance".into()],
        output: ClosedOutputSpec {
            required_fields: vec!["depth".into()],
            allowed_fields: vec!["depth".into()],
            field_types: indexmap::indexmap! {
                "depth".into() => ClosedOutputType::String,
            },
        },
    }
}

pub fn classify_depth_with_provider(
    utterance: &str,
    provider: &mut dyn LlmProvider,
) -> AelioResult<(DepthClass, String)> {
    let spec = classify_depth_prompt_spec();
    let prompt_hash = spec.canonical_hash()?;
    let response = provider.complete(&LlmRequest {
        prompt: spec.render(&indexmap::indexmap! {
            "utterance".into() => utterance.into(),
        })?,
        model: "default".into(),
        temperature: 0.0,
    })?;
    let parsed = spec.parse_output(&response.content)?;
    let depth = match parsed.get("depth").and_then(serde_json::Value::as_str) {
        Some("shallow") => Depth::Shallow,
        Some("boundary") => Depth::Boundary,
        Some("deep") => Depth::Deep,
        _ => {
            return Err(AelioError::new(
                ReasonCode::ParseError,
                "depth must be shallow, boundary, or deep",
            ));
        }
    };
    Ok((
        DepthClass {
            depth,
            margin: 1.0,
            substrate: "closed_model".into(),
        },
        prompt_hash,
    ))
}

pub fn classify_reply_type(utterance: &str) -> ReplyClass {
    let lower = utterance.trim().to_lowercase();
    let generic = [
        "hi",
        "hello",
        "hey",
        "thanks",
        "thank you",
        "bye",
        "ok",
        "okay",
    ];
    if generic.iter().any(|g| lower == *g) {
        return ReplyClass {
            reply_type: ReplyType::Generic,
            margin: 0.9,
        };
    }
    if lower.contains('?') {
        return ReplyClass {
            reply_type: ReplyType::OutputOriented,
            margin: 0.7,
        };
    }
    // looks like providing a value
    if lower
        .chars()
        .all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '+')
    {
        return ReplyClass {
            reply_type: ReplyType::InputOriented,
            margin: 0.85,
        };
    }
    ReplyClass {
        reply_type: ReplyType::Banter,
        margin: 0.5,
    }
}

/// Extract phone / otp with pure patterns first (semantic/LLM escalate later).
pub fn extract_phone(utterance: &str) -> Option<String> {
    let digits: String = utterance.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 10 {
        Some(utterance.trim().to_string())
    } else {
        None
    }
}

pub fn extract_otp(utterance: &str) -> Option<String> {
    let digits: String = utterance.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 6 {
        Some(digits)
    } else {
        None
    }
}

pub fn detect_repair(utterance: &str, _prev: Option<&str>) -> RepairSignal {
    let lower = utterance.to_lowercase();
    let repair_hints = [
        "no, i meant",
        "no i meant",
        "that's wrong",
        "thats wrong",
        "not that",
        "i said",
        "correction",
        "actually",
        "wait, no",
    ];
    if repair_hints.iter().any(|h| lower.contains(h)) {
        return RepairSignal::Repair;
    }
    if lower.contains("forget it") || lower.contains("never mind") || lower.contains("cancel that")
    {
        return RepairSignal::Abandon;
    }
    if lower.contains("this is useless") || lower.contains("stupid") {
        return RepairSignal::Frustration;
    }
    RepairSignal::Normal
}

pub fn detect_consent(utterance: &str) -> &'static str {
    let lower = utterance.trim().to_lowercase();
    let affirm = [
        "yes", "y", "yeah", "yep", "confirm", "ok", "okay", "sure", "do it", "go ahead",
    ];
    let deny = ["no", "n", "nope", "cancel", "stop", "don't", "do not"];
    if affirm
        .iter()
        .any(|a| lower == *a || lower.starts_with(&format!("{a} ")))
    {
        return "affirm";
    }
    if deny
        .iter()
        .any(|d| lower == *d || lower.starts_with(&format!("{d} ")))
    {
        return "deny";
    }
    "unclear"
}

/// ResolveReference returns Ambiguous, never a best guess.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefResolution {
    Id { id: String },
    Ambiguous { candidates: Vec<String> },
    Unresolved,
}

pub fn resolve_reference(utterance: &str, candidates: &[String]) -> RefResolution {
    if candidates.is_empty() {
        return RefResolution::Unresolved;
    }
    let lower = utterance.to_lowercase();
    let hits: Vec<String> = candidates
        .iter()
        .filter(|c| lower.contains(&c.to_lowercase()))
        .cloned()
        .collect();
    match hits.len() {
        0 => {
            if candidates.len() == 1 {
                RefResolution::Id {
                    id: candidates[0].clone(),
                }
            } else {
                RefResolution::Ambiguous {
                    candidates: candidates.to_vec(),
                }
            }
        }
        1 => RefResolution::Id {
            id: hits[0].clone(),
        },
        _ => RefResolution::Ambiguous { candidates: hits },
    }
}

pub fn depth_to_value(d: &DepthClass) -> Value {
    Value::Map(indexmap::indexmap! {
        "depth".into() => Value::str(format!("{:?}", d.depth).to_lowercase()),
        "margin".into() => Value::Float(d.margin),
        "substrate".into() => Value::str(&d.substrate),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentClass {
    /// The declared label (e.g. a capability tag) the utterance most likely intends.
    pub label: String,
    /// top − second score; small means two intents are plausible.
    pub margin: f64,
    /// True when the top intent clears both an absolute-score and a margin bar.
    pub confident: bool,
}

/// `Understand.ClassifyIntent` — declaration-driven, never hardcoded per-vertical buckets.
///
/// Scores the utterance against the tenant's *declared* intent labels (its capability tags) using
/// semantic similarity when a real embedder backs `embedder`, with deterministic token overlap as
/// the free fallback. A CRM's `crm.clients.query` and a clinic's `appointments.book` are both just
/// labels — the engine learns none of them. Returns `None` when nothing scores above the floor, so
/// the caller can fall back to a generic bucket rather than force a wrong intent.
pub fn classify_intent(
    utterance: &str,
    labels: &[String],
    embedder: &dyn Embedder,
) -> Option<IntentClass> {
    const FLOOR: f64 = 0.2;
    const MIN_CONFIDENT_SCORE: f64 = 0.45;
    const MIN_CONFIDENT_MARGIN: f64 = 0.12;

    let utterance_lower = utterance.to_lowercase();
    let utterance_tokens: Vec<String> = utterance_lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    let utterance_vec = embedder.embed(&utterance_lower).ok();

    let mut scored: Vec<(String, f64)> = labels
        .iter()
        .map(|label| {
            (
                label.clone(),
                intent_label_score(label, &utterance_tokens, utterance_vec.as_deref(), embedder),
            )
        })
        .filter(|(_, score)| *score >= FLOOR)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let (label, top) = scored.first().cloned()?;
    let second = scored.get(1).map(|(_, s)| *s).unwrap_or(0.0);
    let margin = top - second;
    Some(IntentClass {
        label,
        margin,
        confident: top >= MIN_CONFIDENT_SCORE && margin >= MIN_CONFIDENT_MARGIN,
    })
}

fn intent_label_score(
    label: &str,
    utterance_tokens: &[String],
    utterance_vec: Option<&[f32]>,
    embedder: &dyn Embedder,
) -> f64 {
    // Deterministic token overlap between the utterance and the label's component words.
    let label_text = label.replace(['.', '_', '-'], " ").to_lowercase();
    let label_tokens: Vec<&str> = label_text.split_whitespace().collect();
    let mut best = 0.0f64;
    if !label_tokens.is_empty() {
        let hits = label_tokens
            .iter()
            .filter(|t| utterance_tokens.iter().any(|u| u == *t))
            .count();
        if hits > 0 {
            best = best.max(hits as f64 / label_tokens.len() as f64);
        }
    }
    // Semantic bridge (only when the embedder's space is meaningful for synonyms). Lexical
    // overlap already carries most labels; skip the remote round-trip when it is decisive.
    if embedder.supports_semantic_equivalence() && best < 0.45 {
        if let (Some(uv), Ok(lv)) = (utterance_vec, embedder.embed(&label_text)) {
            best = best.max(cosine_similarity(uv, &lv));
        }
    }
    best
}

#[cfg(test)]
mod intent_tests {
    use super::*;
    use crate::embedding::HashEmbedder;
    use crate::provider::{LlmResponse, ScriptedLlmProvider};

    fn labels() -> Vec<String> {
        vec![
            "auth.otp.send".into(),
            "crm.clients.query".into(),
            "orders.list".into(),
            "appointments.book".into(),
        ]
    }

    #[test]
    fn intent_matches_declared_capability_by_words() {
        let embedder = HashEmbedder::new(64).unwrap();
        // No hardcoded "orders" bucket — it matches the declared `orders.list` label's own words.
        let got = classify_intent("show my orders please", &labels(), &embedder).unwrap();
        assert_eq!(got.label, "orders.list");

        // A different vertical's label wins for its own words, with the same engine.
        let got = classify_intent("i want to book an appointment", &labels(), &embedder).unwrap();
        assert_eq!(got.label, "appointments.book");
    }

    #[test]
    fn unrelated_utterance_yields_no_intent() {
        let embedder = HashEmbedder::new(64).unwrap();
        // Nothing declared matches → None, so the caller falls back rather than forcing a wrong intent.
        assert!(classify_intent("zzz qqq wubble", &labels(), &embedder).is_none());
    }

    #[test]
    fn no_labels_is_none() {
        let embedder = HashEmbedder::new(64).unwrap();
        assert!(classify_intent("show my orders", &[], &embedder).is_none());
    }

    #[test]
    fn model_clause_split_is_closed_and_bounded() {
        let mut provider = ScriptedLlmProvider::new([Ok(LlmResponse {
            content: r#"{"clauses":["cancel my order","send the receipt"]}"#.into(),
            provider_request_id: None,
            input_tokens: None,
            output_tokens: None,
        })]);
        let (clauses, hash) =
            split_clauses_with_provider("cancel my order and send the receipt", &mut provider)
                .unwrap();
        assert_eq!(clauses, ["cancel my order", "send the receipt"]);
        assert_eq!(hash.len(), 64);

        let mut oversized = ScriptedLlmProvider::new([Ok(LlmResponse {
            content: serde_json::json!({"clauses": ["a", "b", "c", "d", "e"]}).to_string(),
            provider_request_id: None,
            input_tokens: None,
            output_tokens: None,
        })]);
        assert_eq!(
            split_clauses_with_provider("many requests", &mut oversized)
                .unwrap_err()
                .code,
            ReasonCode::BudgetExceeded
        );
    }

    #[test]
    fn model_depth_classification_has_a_closed_vocabulary() {
        let mut provider = ScriptedLlmProvider::new([Ok(LlmResponse {
            content: r#"{"depth":"deep"}"#.into(),
            provider_request_id: None,
            input_tokens: None,
            output_tokens: None,
        })]);
        let (class, hash) = classify_depth_with_provider("réserver demain", &mut provider).unwrap();
        assert_eq!(class.depth, Depth::Deep);
        assert_eq!(class.substrate, "closed_model");
        assert_eq!(hash.len(), 64);

        let mut invalid = ScriptedLlmProvider::new([Ok(LlmResponse {
            content: r#"{"depth":"maybe"}"#.into(),
            provider_request_id: None,
            input_tokens: None,
            output_tokens: None,
        })]);
        assert_eq!(
            classify_depth_with_provider("unknown", &mut invalid)
                .unwrap_err()
                .code,
            ReasonCode::ParseError
        );
    }
}
