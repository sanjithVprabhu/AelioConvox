//! Deterministic admission gates for reused Tier0/Tier1 procedures.
//!
//! Retrieval is only a candidate selector. A candidate is never dispatched until this module has
//! proved that its required bindings are present, no request constraint was dropped, and its
//! discriminating operation still matches the request.

use crate::abilities::learn::path_required_evidence;
use crate::abilities::registry::Registry;
use crate::abilities::residue::residue_check_with_values;
use crate::contract::AbilityPath;
use crate::types::LookupTier;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    Accepted,
    MatchingDisabled,
    Bind,
    Residue,
    Verification,
    BindingCorrectness,
    Structure,
    ImplicitTerm,
    FailClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchDecision {
    pub outcome: Outcome,
    pub gate: Gate,
    pub reason: String,
}

impl MatchDecision {
    fn accepted() -> Self {
        Self {
            outcome: Outcome::Accepted,
            gate: Gate::Accepted,
            reason: "all warm-path gates passed".into(),
        }
    }

    fn rejected(gate: Gate, reason: impl Into<String>) -> Self {
        Self {
            outcome: Outcome::Rejected,
            gate,
            reason: reason.into(),
        }
    }
}

/// Tenant/runtime switch for the red rule: when disabled, lookup results are candidates only and
/// every request takes the cold authoring path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchingConfig {
    pub matching_enabled: bool,
}

impl Default for MatchingConfig {
    fn default() -> Self {
        Self {
            matching_enabled: true,
        }
    }
}

/// Checks a Tier0/Tier1 candidate before any adapter or ability in its path is invoked.
pub fn evaluate_warm_match(
    config: MatchingConfig,
    tier: LookupTier,
    registry: &Registry,
    path: &AbilityPath,
    utterance: &str,
    slots: &IndexMap<String, serde_json::Value>,
) -> MatchDecision {
    if !config.matching_enabled {
        return MatchDecision::rejected(Gate::MatchingDisabled, "matching is disabled");
    }
    if !matches!(tier, LookupTier::Tier0 | LookupTier::Tier1) {
        return MatchDecision::rejected(Gate::FailClosed, "only Tier0/Tier1 may warm-dispatch");
    }

    if contains_implicit_discriminating_term(utterance) {
        return MatchDecision::rejected(
            Gate::ImplicitTerm,
            "implicit aggregation term requires a rendered plan before execution",
        );
    }

    let required = match path_required_evidence(registry, path) {
        Ok(required) => required,
        Err(_) => {
            return MatchDecision::rejected(
                Gate::FailClosed,
                "candidate requirements could not be resolved",
            )
        }
    };
    let missing: Vec<_> = required
        .iter()
        .filter(|name| !slots.contains_key(*name))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return MatchDecision::rejected(
            Gate::Bind,
            format!("required bindings missing: {}", missing.join(", ")),
        );
    }

    if requests_between_range(utterance) && !candidate_supports_between(path) {
        return MatchDecision::rejected(
            Gate::Structure,
            "between-range requires a distinct two-bound skeleton",
        );
    }
    if requests_null_policy(utterance) && !candidate_mentions(path, &["null", "missing"]) {
        return MatchDecision::rejected(
            Gate::Structure,
            "null-policy is structural and is not encoded by this candidate",
        );
    }

    if let (Some(requested), Some(bound)) = (
        requested_aggregation(utterance),
        candidate_aggregation(path),
    ) {
        if requested != bound {
            return MatchDecision::rejected(
                Gate::Verification,
                format!("requested aggregation `{requested}` differs from bound `{bound}`"),
            );
        }
    }

    if let (Some(requested), Some(bound)) =
        (requested_comparison(utterance), candidate_comparison(path))
    {
        if requested != bound {
            return MatchDecision::rejected(
                Gate::BindingCorrectness,
                format!("requested comparison `{requested}` differs from bound `{bound}`"),
            );
        }
    }

    if let Some(requested_field) = slots.get("field").and_then(serde_json::Value::as_str) {
        if let Some(bound_field) = static_arg(path, "field") {
            if requested_field != bound_field {
                return MatchDecision::rejected(
                    Gate::Bind,
                    format!(
                        "requested field `{requested_field}` differs from bound `{bound_field}`"
                    ),
                );
            }
        }
    }

    if requests_entity(utterance, "employees") && candidate_mentions(path, &["people", "person"]) {
        return MatchDecision::rejected(
            Gate::Bind,
            "request source is employees but candidate is bound to people",
        );
    }
    if requests_entity(utterance, "people") && candidate_mentions(path, &["employees", "employee"])
    {
        return MatchDecision::rejected(
            Gate::Bind,
            "request source is people but candidate is bound to employees",
        );
    }

    let residue = residue_check_with_values(utterance, path, slots.values());
    if !residue.is_empty() {
        let fragments = residue
            .iter()
            .map(|fragment| fragment.fragment.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        // A nearest-neighbour hit that cannot account for every constraint must not fall back to
        // "best available". Tier1 has no stronger authority than its score.
        let gate = if tier == LookupTier::Tier1 {
            Gate::FailClosed
        } else {
            Gate::Residue
        };
        return MatchDecision::rejected(gate, format!("unconsumed constraints: {fragments}"));
    }

    MatchDecision::accepted()
}

fn normalized_path(path: &AbilityPath) -> String {
    let mut text = String::new();
    for step in &path.steps {
        text.push(' ');
        text.push_str(&step.ability_id);
        for (key, value) in &step.args {
            text.push(' ');
            text.push_str(key);
            text.push(' ');
            text.push_str(&value.to_string());
        }
    }
    text.to_ascii_lowercase()
}

fn candidate_mentions(path: &AbilityPath, terms: &[&str]) -> bool {
    let path = normalized_path(path);
    terms.iter().any(|term| path.contains(term))
}

fn static_arg<'a>(path: &'a AbilityPath, name: &str) -> Option<&'a str> {
    path.steps
        .iter()
        .find_map(|step| step.args.get(name).and_then(serde_json::Value::as_str))
}

fn requested_aggregation(utterance: &str) -> Option<&'static str> {
    let text = utterance.to_ascii_lowercase();
    if text.contains("median") {
        Some("median")
    } else if text.contains("average") || text.contains("mean") {
        Some("mean")
    } else if text.contains(" sum") {
        Some("sum")
    } else {
        None
    }
}

fn candidate_aggregation(path: &AbilityPath) -> Option<&'static str> {
    let text = normalized_path(path);
    ["median", "mean", "average", "sum"]
        .into_iter()
        .find(|term| text.contains(term))
        .map(|term| if term == "average" { "mean" } else { term })
}

fn requested_comparison(utterance: &str) -> Option<&'static str> {
    if utterance.contains(">=") || utterance.to_ascii_lowercase().contains("at least") {
        Some("gte")
    } else if utterance.contains('>') || utterance.to_ascii_lowercase().contains("over ") {
        Some("gt")
    } else {
        None
    }
}

fn candidate_comparison(path: &AbilityPath) -> Option<&'static str> {
    let text = normalized_path(path);
    if text.contains("gte") || text.contains("ge") || text.contains(">=") {
        Some("gte")
    } else if text.contains("gt") || text.contains('>') {
        Some("gt")
    } else {
        None
    }
}

fn contains_implicit_discriminating_term(utterance: &str) -> bool {
    let text = utterance.to_ascii_lowercase();
    ["typical", "usual", "normal", "middle"].iter().any(|term| {
        text.split(|c: char| !c.is_alphabetic())
            .any(|word| word == *term)
    })
}

fn requests_between_range(utterance: &str) -> bool {
    utterance.to_ascii_lowercase().contains("between ")
}

fn candidate_supports_between(path: &AbilityPath) -> bool {
    candidate_mentions(path, &["between", "range", "min_", "max_"])
}

fn requests_null_policy(utterance: &str) -> bool {
    let text = utterance.to_ascii_lowercase();
    text.contains("including unknown")
        || text.contains("exclude null")
        || text.contains("missing values")
}

fn requests_entity(utterance: &str, entity: &str) -> bool {
    utterance
        .split(|c: char| !c.is_alphabetic())
        .any(|word| word.eq_ignore_ascii_case(entity))
}
