//! Semantic term resolution anchored to declared attributes with polarity.
//! Antonyms are distributionally close — never free-float over text.

use crate::abilities::judge::{confidence, should_escalate};
use crate::abilities::learn::cosine_similarity;
use crate::embedding::Embedder;
use crate::provider::{
    ClosedOutputSpec, ClosedOutputType, LlmProvider, LlmRequest, PromptSlotSpec, PromptSpec,
};
use crate::tenant::{AttributeSpec, Polarity};
use crate::types::{AelioError, AelioResult, ReasonCode, Sensitivity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPlanFragment {
    pub attribute: String,
    pub direction: SortDir,
    pub limit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TermResolution {
    Resolved {
        plan: QueryPlanFragment,
        margin: f64,
    },
    Confirm {
        attribute: String,
        margin: f64,
        runners_up: Vec<(String, f64)>,
    },
    Unresolved,
}

/// Build the exact sortable plan a user explicitly confirmed. This does not rescore the original
/// language, so a parked confirmation cannot drift after a restart or catalog synonym update.
pub fn confirmed_plan(
    attribute: &str,
    want_most: bool,
    limit: u64,
    attributes: &[AttributeSpec],
) -> Option<QueryPlanFragment> {
    let declared = attributes
        .iter()
        .find(|candidate| candidate.name == attribute && candidate.sortable)?;
    let direction = match (want_most, declared.polarity) {
        (true, Polarity::HighMeansMore) => SortDir::Desc,
        (true, Polarity::HighMeansLess) => SortDir::Asc,
        (false, Polarity::HighMeansMore) => SortDir::Asc,
        (false, Polarity::HighMeansLess) => SortDir::Desc,
    };
    Some(QueryPlanFragment {
        attribute: attribute.into(),
        direction,
        limit,
    })
}

/// Similarity of a term to an attribute, anchored to the attribute's *declared* anchors and
/// learned synonyms — never to hardcoded, vertical-specific knowledge baked into the engine
/// (decisions P2/M1). Two signals combine, taking the stronger:
///   1. deterministic lexical overlap (exact / substring / token / bigram) — the free path that
///      resolves declared vocabulary without a model, and
///   2. embedding cosine against each anchor — the semantic path that lets a *novel* word ("avaricious")
///      bridge to a declared anchor ("greedy") when a real embedding model backs `embedder`. With a
///      bag-of-hash embedder this is inert, so undeclared novel words fall to the confirm/learn loop.
fn score_term(term: &str, attr: &AttributeSpec, embedder: &dyn Embedder) -> f64 {
    let t = term.to_lowercase();
    let mut best = 0.0f64;
    let term_vec = embedder
        .supports_semantic_equivalence()
        .then(|| embedder.embed(&t).ok())
        .flatten();
    for a in attr.anchors.iter().chain(attr.learned.iter()) {
        let a = a.to_lowercase();
        if t == a {
            best = best.max(1.0);
        } else if t.chars().count() >= 3
            && a.chars().count() >= 3
            && (a.contains(&t) || t.contains(&a))
        {
            best = best.max(0.85);
        } else {
            // token overlap
            let ta: Vec<_> = t.split_whitespace().collect();
            let aa: Vec<_> = a.split_whitespace().collect();
            let inter = ta.iter().filter(|x| aa.contains(x)).count();
            if inter > 0 {
                best = best.max(0.5 + 0.1 * inter as f64);
            }
            // crude char bigram Jaccard
            let j = bigram_jaccard(&t, &a);
            // Treat this only as a typo/inflection bridge. Weak shared character fragments are
            // not evidence of semantic equivalence and must not select a business attribute.
            if j >= 0.65 {
                best = best.max(j * 0.9);
            }
        }
        // Semantic bridge to the declared anchor.
        if let (Some(tv), Ok(av)) = (term_vec.as_ref(), embedder.embed(&a)) {
            best = best.max(cosine_similarity(tv, &av));
        }
    }
    // Attribute names are declaration data too, but short substrings such as "id" must not
    // authorize a field. Accept the full normalized name, or all of its complete tokens.
    let attribute_phrase = attr.name.replace('_', " ").to_ascii_lowercase();
    if t == attribute_phrase {
        best = best.max(1.0);
    } else {
        let term_tokens: std::collections::HashSet<_> = t.split_whitespace().collect();
        let attribute_tokens: Vec<_> = attribute_phrase.split_whitespace().collect();
        if !attribute_tokens.is_empty()
            && attribute_tokens
                .iter()
                .all(|token| term_tokens.contains(token))
        {
            best = best.max(0.85);
        }
    }
    best
}

fn bigram_jaccard(a: &str, b: &str) -> f64 {
    fn bigrams(s: &str) -> std::collections::HashSet<String> {
        let cs: Vec<char> = s.chars().collect();
        let mut set = std::collections::HashSet::new();
        if cs.len() < 2 {
            set.insert(s.to_string());
            return set;
        }
        for i in 0..cs.len() - 1 {
            set.insert(format!("{}{}", cs[i], cs[i + 1]));
        }
        set
    }
    let aa = bigrams(a);
    let bb = bigrams(b);
    let inter = aa.intersection(&bb).count() as f64;
    let union = aa.union(&bb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

pub fn resolve_term(
    term: &str,
    want_most: bool,
    limit: u64,
    attributes: &[AttributeSpec],
    margin_threshold: f64,
    embedder: &dyn Embedder,
) -> TermResolution {
    let mut scored: Vec<(String, f64, Polarity, bool)> = attributes
        .iter()
        .map(|a| {
            (
                a.name.clone(),
                score_term(term, a, embedder),
                a.polarity,
                a.sortable,
            )
        })
        .filter(|(_, s, _, _)| *s > 0.1)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    if scored.is_empty() {
        return TermResolution::Unresolved;
    }

    let scores: Vec<f64> = scored.iter().map(|s| s.1).collect();
    let conf = confidence(&scores);

    if should_escalate(&conf, margin_threshold) {
        let runners: Vec<(String, f64)> =
            scored.iter().take(3).map(|s| (s.0.clone(), s.1)).collect();
        return TermResolution::Confirm {
            attribute: scored[0].0.clone(),
            margin: conf.margin,
            runners_up: runners,
        };
    }

    let (name, _s, polarity, sortable) = &scored[0];
    if !sortable {
        return TermResolution::Unresolved;
    }

    let direction = match (want_most, polarity) {
        (true, Polarity::HighMeansMore) => SortDir::Desc,
        (true, Polarity::HighMeansLess) => SortDir::Asc,
        (false, Polarity::HighMeansMore) => SortDir::Asc,
        (false, Polarity::HighMeansLess) => SortDir::Desc,
    };

    TermResolution::Resolved {
        plan: QueryPlanFragment {
            attribute: name.clone(),
            direction,
            limit,
        },
        margin: conf.margin,
    }
}

/// Free, deterministic adapter for "most/least" ranking language. It deliberately does not know
/// tenant entity nouns: the complete phrase after the operator is resolved against declared
/// attribute anchors. Other languages/forms can use the closed structured-understanding path.
pub fn parse_rank_query(utterance: &str) -> Option<(String, u64, bool)> {
    let lower = utterance.to_lowercase();
    let want_most = if lower.contains("least") {
        false
    } else if lower.contains("most") {
        true
    } else {
        return None;
    };
    // number
    let limit = lower
        .split_whitespace()
        .find_map(|w| w.parse::<u64>().ok())
        .unwrap_or(10);
    // term after most/least
    let key = if want_most { "most" } else { "least" };
    if let Some(pos) = lower.find(key) {
        let after = lower[pos + key.len()..].trim();
        let term = after.trim_matches(|character: char| {
            character.is_ascii_punctuation() && character != '-' && character != '_'
        });
        if !term.is_empty() {
            return Some((term.to_string(), limit, want_most));
        }
    }
    None
}

pub fn rank_extract_prompt_spec() -> PromptSpec {
    PromptSpec {
        id: "aelio.understand.rank".into(),
        version: "1".into(),
        instruction: concat!(
            "Extract a ranking request without choosing a business attribute. ",
            "Return the user's descriptive term, an integer limit encoded as a string, ",
            "and operator exactly `most` or `least`."
        )
        .into(),
        slots: vec![PromptSlotSpec {
            name: "utterance".into(),
            required: true,
            sensitivity: Sensitivity::Pii,
        }],
        max_chars: 8_000,
        max_tokens: 2_000,
        truncation_order: vec!["utterance".into()],
        output: ClosedOutputSpec {
            required_fields: vec!["term".into(), "limit".into(), "operator".into()],
            allowed_fields: vec!["term".into(), "limit".into(), "operator".into()],
            field_types: indexmap::indexmap! {
                "term".into() => ClosedOutputType::String,
                "limit".into() => ClosedOutputType::String,
                "operator".into() => ClosedOutputType::String,
            },
        },
    }
}

pub fn parse_rank_query_with_provider(
    utterance: &str,
    provider: &mut dyn LlmProvider,
) -> AelioResult<((String, u64, bool), String)> {
    let spec = rank_extract_prompt_spec();
    let prompt_hash = spec.canonical_hash()?;
    let response = provider.complete(&LlmRequest {
        prompt: spec.render(&indexmap::indexmap! {
            "utterance".into() => utterance.into(),
        })?,
        model: "default".into(),
        temperature: 0.0,
    })?;
    let output = spec.parse_output(&response.content)?;
    let term = output
        .get("term")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|term| !term.is_empty() && term.len() <= 512)
        .ok_or_else(|| AelioError::new(ReasonCode::ParseError, "rank term is invalid"))?;
    let limit = output
        .get("limit")
        .and_then(serde_json::Value::as_str)
        .and_then(|limit| limit.parse::<u64>().ok())
        .filter(|limit| (1..=1_000).contains(limit))
        .ok_or_else(|| AelioError::new(ReasonCode::ParseError, "rank limit is outside 1..=1000"))?;
    let want_most = match output.get("operator").and_then(serde_json::Value::as_str) {
        Some("most") => true,
        Some("least") => false,
        _ => {
            return Err(AelioError::new(
                ReasonCode::ParseError,
                "rank operator must be `most` or `least`",
            ))
        }
    };
    Ok(((term.into(), limit, want_most), prompt_hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;
    use crate::provider::ScriptedLlmProvider;

    /// Models a real embedding neighbourhood: "avaricious" sits near *both* "greedy" and "generous"
    /// (antonyms are distributionally close). This is the honest substrate for the antonym trap —
    /// the ambiguity comes from the embedding space, not from vertical-specific engine code.
    struct NeighbourhoodEmbedder;
    impl Embedder for NeighbourhoodEmbedder {
        fn dimension(&self) -> usize {
            4
        }
        fn embed(&self, text: &str) -> crate::AelioResult<Vec<f32>> {
            let v = match text {
                "greedy" => vec![1.0, 0.0, 0.0, 0.0],
                "generous" => vec![0.0, 1.0, 0.0, 0.0],
                // Close to both greedy and generous → the trap.
                "avaricious" => vec![0.79, 0.74, 0.0, 0.0],
                "giving" => vec![0.0, 1.0, 0.0, 0.0],
                _ => vec![0.0, 0.0, 0.0, 1.0],
            };
            Ok(v)
        }

        fn supports_semantic_equivalence(&self) -> bool {
            true
        }
    }

    fn attrs() -> Vec<AttributeSpec> {
        vec![
            AttributeSpec {
                name: "discount_pressure_score".into(),
                anchors: vec![
                    "greedy".into(),
                    "haggler".into(),
                    "price-sensitive".into(),
                    "always negotiating".into(),
                ],
                learned: vec![],
                polarity: Polarity::HighMeansMore,
                sortable: true,
                query_capability: Some("clients.query".into()),
            },
            AttributeSpec {
                name: "generosity_index".into(),
                anchors: vec!["generous".into(), "giving".into()],
                learned: vec![],
                polarity: Polarity::HighMeansMore,
                sortable: true,
                query_capability: Some("clients.query".into()),
            },
        ]
    }

    #[test]
    fn avaricious_needs_confirm_when_margin_thin() {
        // A novel word never declared as a synonym resolves *only* through the embedding bridge to
        // declared anchors — and lands thin between an attribute and its antonym, so it confirms.
        let r = resolve_term(
            "avaricious",
            true,
            10,
            &attrs(),
            0.15,
            &NeighbourhoodEmbedder,
        );
        match r {
            TermResolution::Confirm { attribute, .. } => {
                assert_eq!(attribute, "discount_pressure_score");
            }
            TermResolution::Resolved { plan, margin } if margin < 0.15 => {
                assert_eq!(plan.attribute, "discount_pressure_score");
            }
            other => panic!("expected confirm or thin resolve, got {other:?}"),
        }
    }

    /// An embedder that bridges nothing — isolates the lexical/declared paths so we can prove the
    /// engine never invents a vertical-specific match on its own.
    struct ZeroEmbedder;
    impl Embedder for ZeroEmbedder {
        fn dimension(&self) -> usize {
            4
        }
        fn embed(&self, _text: &str) -> crate::AelioResult<Vec<f32>> {
            Ok(vec![0.0, 0.0, 0.0, 0.0])
        }
    }

    #[test]
    fn engine_invents_no_match_for_unrelated_terms() {
        // A term that shares no anchor, no declared synonym, and (under a null embedder) no semantic
        // bridge must stay Unresolved. If a hardcoded synonym table still lived in the engine, a word
        // like this could never be guaranteed unresolved. Proves the vertical hardcode is gone.
        assert!(matches!(
            resolve_term("wuzzlorb", true, 10, &attrs(), 0.15, &ZeroEmbedder),
            TermResolution::Unresolved
        ));
    }

    #[test]
    fn offline_hash_embeddings_cannot_authorize_novel_semantic_terms() {
        let embedder = HashEmbedder::new(32).unwrap();
        assert!(matches!(
            resolve_term("avaricious", true, 10, &attrs(), 0.15, &embedder),
            TermResolution::Unresolved
        ));
    }

    #[test]
    fn partial_attribute_name_tokens_do_not_resolve() {
        let embedder = HashEmbedder::new(32).unwrap();
        assert!(matches!(
            resolve_term("index", true, 10, &attrs(), 0.15, &embedder),
            TermResolution::Unresolved
        ));
    }

    #[test]
    fn model_rank_extraction_is_closed_and_bounded() {
        let mut provider = ScriptedLlmProvider::deterministic().with_fallback(
            "aelio.understand.rank",
            r#"{"term":"avaricieux","limit":"12","operator":"most"}"#,
        );
        let ((term, limit, want_most), prompt_hash) =
            parse_rank_query_with_provider("les clients les plus avaricieux", &mut provider)
                .unwrap();
        assert_eq!(term, "avaricieux");
        assert_eq!(limit, 12);
        assert!(want_most);
        assert_eq!(prompt_hash.len(), 64);

        let mut invalid = ScriptedLlmProvider::deterministic().with_fallback(
            "aelio.understand.rank",
            r#"{"term":"x","limit":"1000000","operator":"drop_table"}"#,
        );
        assert!(parse_rank_query_with_provider("unsafe", &mut invalid).is_err());
    }

    #[test]
    fn declared_synonym_resolves_without_any_embedder_bridge() {
        // The tenant (or the learn loop) declares "avaricious" as a learned synonym → it resolves
        // deterministically, no model needed. This is the vertical-agnostic path: knowledge is data.
        let mut declared = attrs();
        declared[0].learned.push("avaricious".into());
        let embedder = HashEmbedder::new(32).unwrap();
        match resolve_term("avaricious", true, 10, &declared, 0.15, &embedder) {
            TermResolution::Resolved { plan, .. } => {
                assert_eq!(plan.attribute, "discount_pressure_score");
            }
            other => panic!("expected resolved via declared synonym, got {other:?}"),
        }
    }

    #[test]
    fn polarity_drives_direction() {
        // Force high margin by using exact anchor
        let r = resolve_term("greedy", true, 10, &attrs(), 0.05, &NeighbourhoodEmbedder);
        match r {
            TermResolution::Resolved { plan, .. } => {
                assert_eq!(plan.attribute, "discount_pressure_score");
                assert_eq!(plan.direction, SortDir::Desc);
                assert_eq!(plan.limit, 10);
            }
            other => panic!("expected resolved, got {other:?}"),
        }
    }
}
