//! Residue check — verifies that every constraint-shaped fragment of an utterance (a number, a
//! quoted value, a named proper noun) was actually consumed somewhere in the args of the path
//! about to execute. Modeled on the check documented in `FLAGS.md` F-032: a skeleton/path with
//! every *required* hole filled can still silently drop a constraint the utterance stated — "...
//! in Bangalore" binding cleanly to a filter-aggregate shape that never mentions the city — and
//! the rest of the request still answers confidently and wrong.
//!
//! Heuristic and high-recall by design: a false positive costs one re-triage back through the
//! cold path; a false negative lets a stated constraint vanish. It never asserts a path is
//! *correct*, only that nothing utterance-shaped and checkable was dropped.
//!
//! **Scope of this pass.** Wired only into the `ProposePath` (cold, model-authored) branch,
//! where `PathStep::args` are the literal values the model just emitted from this exact
//! utterance. Tier0/Tier1 (promoted-path reuse) and Tier2 (`compose`) bind tool parameters
//! dynamically via `abilities::bind::resolve_all` against `input.slots` at execution time rather
//! than storing literal values in the path itself, so a residue check there needs a different
//! check point (against resolved slots, not `PathStep::args`) — left as documented follow-up in
//! `FLAGS.md` F-032 rather than guessed at here.

use crate::contract::AbilityPath;
use regex::Regex;
use std::sync::OnceLock;

/// A constraint-shaped fragment extracted from the utterance that could not be traced into any
/// arg of the path about to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnconsumedFragment {
    pub fragment: String,
}

fn number_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d+(?:\.\d+)?").expect("static pattern"))
}

fn quoted_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"["']([^"']{2,})["']"#).expect("static pattern"))
}

/// Extracts candidate constraint fragments: numeric literals, quoted values, and capitalized
/// proper-noun-like words (skipping the utterance's first word, which is ordinary sentence-case
/// far more often than it is a named value). Deliberately narrow — this system has no
/// schema-graph of known field/entity vocabulary to check against (see F-032), so this stays to
/// the fragment classes that are cheap to extract without one.
pub fn extract_constraint_fragments(utterance: &str) -> Vec<String> {
    let mut out = Vec::new();
    for capture in quoted_pattern().captures_iter(utterance) {
        out.push(capture[1].trim().to_string());
    }
    for m in number_pattern().find_iter(utterance) {
        out.push(m.as_str().to_string());
    }
    for (index, word) in utterance.split_whitespace().enumerate() {
        if index == 0 {
            continue;
        }
        let cleaned: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
        if cleaned.chars().count() < 3 {
            continue;
        }
        let mut chars = cleaned.chars();
        let Some(first) = chars.next() else { continue };
        if first.is_uppercase() && chars.clone().all(|c| c.is_lowercase()) {
            out.push(cleaned);
        }
    }
    // A location/value qualifier is constraint-shaped even when written in lower case. This is
    // deliberately narrow: it closes the "in Bangalore" clause-drop class without treating every
    // ordinary word as a constraint.
    let words: Vec<_> = utterance.split_whitespace().collect();
    for pair in words.windows(2) {
        if pair[0].eq_ignore_ascii_case("in") {
            let cleaned: String = pair[1].chars().filter(|c| c.is_alphanumeric()).collect();
            if cleaned.chars().count() >= 3 && !cleaned.chars().all(|c| c.is_ascii_digit()) {
                out.push(cleaned);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Flattens every arg value across every step of a path into one lowercase, searchable blob.
fn path_args_blob(path: &AbilityPath) -> String {
    let mut blob = String::new();
    for step in &path.steps {
        for value in step.args.values() {
            blob.push(' ');
            blob.push_str(&value.to_string());
        }
    }
    blob.to_lowercase()
}

/// Checks that every constraint fragment extracted from `utterance` appears somewhere in the
/// args of `path`. Returns the fragments that did not — empty means the check passed. A fragment
/// found as a substring of *any* arg value anywhere in the path counts as consumed; this never
/// rejects a path over a phrasing mismatch it cannot actually prove is wrong, only over a
/// constraint that is nowhere at all.
pub fn residue_check(utterance: &str, path: &AbilityPath) -> Vec<UnconsumedFragment> {
    residue_check_with_values(utterance, path, std::iter::empty::<&serde_json::Value>())
}

/// Warm candidates bind parameters at dispatch from slots, so their stored path arguments alone
/// are not a complete consumption record. Check the resolved values as well as literal path args.
pub fn residue_check_with_values<'a>(
    utterance: &str,
    path: &AbilityPath,
    values: impl IntoIterator<Item = &'a serde_json::Value>,
) -> Vec<UnconsumedFragment> {
    let fragments = extract_constraint_fragments(utterance);
    if fragments.is_empty() {
        return Vec::new();
    }
    let mut blob = path_args_blob(path);
    for value in values {
        blob.push(' ');
        blob.push_str(&value.to_string().to_lowercase());
    }
    fragments
        .into_iter()
        .filter(|fragment| !blob.contains(&fragment.to_lowercase()))
        .map(|fragment| UnconsumedFragment { fragment })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::PathStep;
    use indexmap::IndexMap;

    fn path_with_args(pairs: &[(&str, serde_json::Value)]) -> AbilityPath {
        let mut args = IndexMap::new();
        for (key, value) in pairs {
            args.insert((*key).to_string(), value.clone());
        }
        AbilityPath {
            steps: vec![PathStep {
                ability_id: "test.ability@1".into(),
                args,
            }],
        }
    }

    #[test]
    fn dropped_filter_value_is_flagged() {
        let path = path_with_args(&[("min_age", serde_json::json!(28))]);
        let unconsumed = residue_check("average height of people over 28 in Bangalore", &path);
        assert_eq!(
            unconsumed,
            vec![UnconsumedFragment {
                fragment: "Bangalore".into()
            }]
        );
    }

    #[test]
    fn fully_bound_path_has_no_residue() {
        let path = path_with_args(&[
            ("min_age", serde_json::json!(28)),
            ("city", serde_json::json!("Bangalore")),
        ]);
        let unconsumed = residue_check("average height of people over 28 in Bangalore", &path);
        assert!(unconsumed.is_empty());
    }

    #[test]
    fn quoted_value_must_also_be_consumed() {
        let path = path_with_args(&[("status", serde_json::json!("open"))]);
        let unconsumed = residue_check("show tickets tagged 'urgent' that are open", &path);
        assert_eq!(
            unconsumed,
            vec![UnconsumedFragment {
                fragment: "urgent".into()
            }]
        );
    }

    #[test]
    fn no_numeric_or_named_fragments_means_nothing_to_check() {
        let path = path_with_args(&[]);
        let unconsumed = residue_check("say hello to the team", &path);
        assert!(unconsumed.is_empty());
    }

    #[test]
    fn sentence_initial_capitalization_is_not_a_false_positive() {
        // "Show" is capitalized only because it starts the sentence, not because it names a
        // constraint value — the first word is always skipped.
        let path = path_with_args(&[]);
        let unconsumed = residue_check("Show me the team roster", &path);
        assert!(unconsumed.is_empty());
    }
}
