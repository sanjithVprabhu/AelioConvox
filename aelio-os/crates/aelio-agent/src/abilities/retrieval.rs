//! Ability retrieval — shortlist declared abilities before ProposePath builds its prompt.
//!
//! ProposePath serializes every declared ability, with its tools and params, into one prompt. That
//! is fine for a handful of abilities and breaks down as a tenant's catalog grows: the prompt runs
//! into context limits, and selection accuracy degrades well before it does.
//!
//! Retrieval narrows the candidate set; it never decides. Three jobs stay separate:
//!
//! - **retrieval** (here) answers *which abilities are plausibly relevant*, optimizing recall,
//! - **ProposePath** selects one path from that set, and the selection is typechecked,
//! - the **tool-call gate** authorizes the resulting effect (policy, binding, redaction).
//!
//! Because `propose_path_with_provider` also uses the id list as its allow-list, shortlisting can
//! only *narrow* what the model may choose. A poor shortlist therefore costs a failed-closed
//! proposal, never a wrong effect.
//!
//! Three rules follow from optimizing recall rather than precision. A near-tie is kept, not
//! resolved — the opposite of [`crate::blocks::term_resolve`], which escalates a thin margin to
//! the user, because there the score selects and here it only nominates. When scoring has no
//! opinion at all, retrieval yields the whole group rather than an arbitrary slice: an over-long
//! prompt degrades, an empty one fails outright. And tenant abilities are budgeted separately from
//! the first-party `std.*` catalog, so a utility tool that happens to share a word with the
//! utterance can never displace the tenant capability the turn actually needs.

use crate::abilities::learn::cosine_similarity;
use crate::abilities::registry::{Registry, STANDARD_TOOL_PREFIX};
use crate::embedding::Embedder;

#[derive(Debug, Clone, Copy)]
pub struct RetrievalConfig {
    /// Catalogs at or below this size are passed through whole. Retrieval exists to keep the
    /// prompt tractable; below the threshold there is nothing to gain and a candidate to lose.
    pub always_inline_below: usize,
    /// How many tenant and engine abilities to keep. These are the actual decision space, so the
    /// reserve is generous and normally admits all of them.
    pub core_reserve: usize,
    /// How many first-party `std.*` utility abilities to admit. This group is large, uniform, and
    /// mostly irrelevant to any one turn, so it is sampled by relevance rather than carried whole.
    pub utility_budget: usize,
    /// Keep an ability past its budget when it scores within this of the cutoff, so the boundary
    /// never falls between two candidates the scorer cannot tell apart.
    pub tie_band: f64,
    /// Floor for tie expansion, as a fraction of the group's top score. Without it a flat tail of
    /// near-identical low scores sits inside `tie_band` and drags the whole group back in.
    pub min_relative_score: f64,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            always_inline_below: 24,
            core_reserve: 24,
            utility_budget: 8,
            tie_band: 0.05,
            min_relative_score: 0.35,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RetrievalOutcome {
    /// Abilities to offer ProposePath, most relevant first.
    pub ability_ids: Vec<String>,
    /// How many abilities were declared before shortlisting.
    pub considered: usize,
    /// False when the catalog was small enough to pass through untouched.
    pub applied: bool,
}

/// Shortlist declared abilities by relevance to the utterance.
pub fn shortlist_abilities(
    utterance: &str,
    ability_ids: &[String],
    registry: &Registry,
    embedder: &dyn Embedder,
    config: &RetrievalConfig,
) -> RetrievalOutcome {
    let considered = ability_ids.len();
    if considered <= config.always_inline_below {
        return RetrievalOutcome {
            ability_ids: ability_ids.to_vec(),
            considered,
            applied: false,
        };
    }

    // The two groups are budgeted separately and never compete. A tenant capability must not be
    // pushed out of the prompt by a `std.*` utility that merely shares a word with the utterance:
    // the tenant's abilities are the decision space, the utility catalog is scaffolding.
    let (utility, core): (Vec<&String>, Vec<&String>) = ability_ids
        .iter()
        .partition(|id| id.starts_with(STANDARD_TOOL_PREFIX));

    let utterance_lower = utterance.to_lowercase();
    let utterance_tokens = content_tokens(&utterance_lower);
    // Semantic scoring is for the tenant/core decision space only, and only when a real model
    // backs the embedder. The `std.*` utility catalog is large, uniform, and named for lexical
    // match — embedding every descriptor over HTTP (one round-trip each) stalls the turn under
    // the gateway wall-clock budget. Lexical sampling is enough for that group.
    let core_needs_rank = core.len() > config.core_reserve;
    let utterance_vec = (core_needs_rank && embedder.supports_semantic_equivalence())
        .then(|| embedder.embed(&utterance_lower).ok())
        .flatten();

    let score_core = |id: &String| {
        relevance(
            &ability_descriptor(registry, id),
            &utterance_tokens,
            utterance_vec.as_deref(),
            embedder,
            /*use_semantic=*/ true,
        )
    };
    let score_utility = |id: &String| {
        relevance(
            &ability_descriptor(registry, id),
            &utterance_tokens,
            None,
            embedder,
            /*use_semantic=*/ false,
        )
    };

    let mut kept = take_by_relevance(
        &core,
        config.core_reserve,
        config,
        &score_core,
        NoSignalPolicy::KeepAll,
    );
    kept.extend(take_by_relevance(
        &utility,
        config.utility_budget,
        config,
        &score_utility,
        // Utilities are scaffolding: when nothing matches, keep a stable sample — not the whole
        // catalog — so ProposePath's prompt stays tractable.
        NoSignalPolicy::KeepBudget,
    ));

    // Preserve the caller's declaration order so the prompt is stable and diffable.
    let mut ability_ids_out: Vec<String> = ability_ids
        .iter()
        .filter(|id| kept.contains(id))
        .cloned()
        .collect();
    if ability_ids_out.is_empty() {
        ability_ids_out = ability_ids.to_vec();
    }
    let applied = ability_ids_out.len() < considered;

    RetrievalOutcome {
        ability_ids: ability_ids_out,
        considered,
        applied,
    }
}

/// What to do when every candidate in a group scores ≤ 0.
#[derive(Debug, Clone, Copy)]
enum NoSignalPolicy {
    /// Hand the whole group through. Used for tenant/core abilities: missing the only viable
    /// ability fails the turn outright.
    KeepAll,
    /// Keep a stable `budget`-sized sample. Used for `std.*` utilities, which are scaffolding.
    KeepBudget,
}

/// Rank one group and keep the best `budget`, plus anything tied with the cutoff.
fn take_by_relevance(
    group: &[&String],
    budget: usize,
    config: &RetrievalConfig,
    score_of: &impl Fn(&String) -> f64,
    on_no_signal: NoSignalPolicy,
) -> Vec<String> {
    if group.len() <= budget {
        return group.iter().map(|id| (*id).clone()).collect();
    }
    let mut scored: Vec<(String, f64)> = group
        .iter()
        .map(|id| ((*id).clone(), score_of(id)))
        .collect();
    // Ties break on id so an identical catalog and utterance always yield an identical prompt.
    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    if scored.iter().all(|(_, score)| *score <= 0.0) {
        return match on_no_signal {
            NoSignalPolicy::KeepAll => group.iter().map(|id| (*id).clone()).collect(),
            NoSignalPolicy::KeepBudget => {
                let mut ids: Vec<String> = group.iter().map(|id| (*id).clone()).collect();
                ids.sort();
                ids.truncate(budget);
                ids
            }
        };
    }

    let top = scored[0].1;
    let cutoff = scored[budget.max(1).min(scored.len()) - 1].1;
    let tie_floor = (cutoff - config.tie_band).max(top * config.min_relative_score);
    // The tie band stops the boundary falling between two candidates the scorer cannot separate.
    // When an entire group scores alike the band carries no information at all, so it would admit
    // everything and the budget would never bind. Cap the expansion: ordering is deterministic, so
    // the cut is at least stable across runs.
    let ceiling = budget.saturating_mul(2);
    scored
        .into_iter()
        .enumerate()
        .filter(|(rank, (_, score))| *rank < budget || *score >= tie_floor)
        .take(ceiling)
        .map(|(_, (id, _))| id)
        .collect()
}

/// Flatten everything declared about an ability into one searchable text: its own id, and for each
/// tool it depends on, the tool id, name, capability tags, parameter names, and the declared
/// meaning of each output field.
pub fn ability_descriptor(registry: &Registry, ability_id: &str) -> String {
    let mut parts = vec![ability_id.replace(['.', '_', '-'], " ")];
    let Some(contract) = registry.abilities.get(ability_id) else {
        return parts.join(" ").to_lowercase();
    };
    for tool_id in &contract.tool_deps {
        let Some(tool) = registry.tools.get(tool_id) else {
            continue;
        };
        parts.push(tool.id.replace(['.', '_', '-'], " "));
        parts.push(tool.name.replace(['.', '_', '-'], " "));
        for tag in &tool.capability_tags {
            parts.push(tag.replace(['.', '_', '-'], " "));
        }
        for param in &tool.params {
            parts.push(param.name.replace(['.', '_', '-'], " "));
        }
        for field in tool.output_semantics.fields.values() {
            parts.push(field.meaning.clone());
        }
    }
    parts.join(" ").to_lowercase()
}

/// Coverage of the utterance by the descriptor, or embedding cosine, whichever is stronger.
///
/// Coverage is normalized by the *utterance*, not the descriptor: the question is how much of what
/// the user asked this ability speaks to. Normalizing by the descriptor would punish tools simply
/// for declaring many parameters.
fn relevance(
    descriptor: &str,
    utterance_tokens: &[String],
    utterance_vec: Option<&[f32]>,
    embedder: &dyn Embedder,
    use_semantic: bool,
) -> f64 {
    let mut best = 0.0f64;
    if !utterance_tokens.is_empty() {
        let descriptor_tokens = content_tokens(descriptor);
        let hits = utterance_tokens
            .iter()
            .filter(|token| descriptor_tokens.iter().any(|d| d == *token))
            .count();
        if hits > 0 {
            best = best.max(hits as f64 / utterance_tokens.len() as f64);
        }
    }
    // Semantic bridge — only when the caller opts in *and* the embedder's space is meaningful.
    // Bag-of-hash is inert for synonym bridging; gateway models are live but must not be invoked
    // once per ability on the hot path (see utility shortlist).
    if use_semantic && embedder.supports_semantic_equivalence() {
        if let (Some(uv), Ok(dv)) = (utterance_vec, embedder.embed(descriptor)) {
            best = best.max(cosine_similarity(uv, &dv));
        }
    }
    best
}

/// Short function words carry no retrieval signal and only compress the score range.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "you", "your", "can", "could", "would", "please",
    "want", "need", "get", "got", "are", "was", "were", "have", "has", "from", "our", "their",
];

fn content_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| token.len() >= 3 && !STOPWORDS.contains(token))
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::AbilityContract;
    use crate::embedding::HashEmbedder;
    use crate::tenant::{OutputField, OutputSpec, ToolEffect, ToolSpec};
    use crate::types::{AelioResult, Sensitivity};
    use indexmap::IndexMap;

    fn tool(id: &str, tags: &[&str], meaning: &str) -> ToolSpec {
        ToolSpec {
            id: id.into(),
            name: id.into(),
            version: "1".into(),
            capability_tags: tags.iter().map(|t| (*t).into()).collect(),
            contract: Some(crate::tenant::ToolContract::complete_read(
                "retrieval_result",
            )),
            effect: Some(ToolEffect::Read),
            effectful: false,
            idempotent: true,
            dry_run_available: true,
            params: vec![],
            output_semantics: OutputSpec {
                fields: IndexMap::from([(
                    "result".into(),
                    OutputField {
                        path: "result".into(),
                        type_name: "string".into(),
                        sensitivity: Sensitivity::None,
                        meaning: meaning.into(),
                    },
                )]),
                role_hint: None,
            },
            continuations: vec![],
            errors: vec![],
        }
    }

    /// Registry with `count` filler abilities plus the named ones, so the catalog clears the
    /// inline threshold and retrieval actually engages.
    fn registry_with(named: &[(&str, &[&str], &str)], filler: usize) -> (Registry, Vec<String>) {
        let mut registry = Registry::default();
        let mut ids = Vec::new();
        for (id, tags, meaning) in named {
            registry.register_tool(tool(id, tags, meaning)).unwrap();
            let mut contract = AbilityContract::pure(*id);
            contract.tool_deps = vec![(*id).into()];
            registry.abilities.insert((*id).into(), contract);
            ids.push((*id).to_string());
        }
        for index in 0..filler {
            let id = format!("filler.unrelated.op{index:03}");
            registry
                .register_tool(tool(&id, &["filler"], "unrelated filler capability"))
                .unwrap();
            let mut contract = AbilityContract::pure(&id);
            contract.tool_deps = vec![id.clone()];
            registry.abilities.insert(id.clone(), contract);
            ids.push(id);
        }
        (registry, ids)
    }

    #[test]
    fn small_catalog_passes_through_untouched() {
        let (registry, ids) = registry_with(&[("crm.clients.query", &["crm"], "client rows")], 3);
        let outcome = shortlist_abilities(
            "show me my clients",
            &ids,
            &registry,
            &HashEmbedder::new(64).expect("embedder"),
            &RetrievalConfig::default(),
        );
        assert!(!outcome.applied, "small catalogs must not be filtered");
        assert_eq!(outcome.ability_ids, ids);
    }

    #[test]
    fn large_catalog_shortlists_and_keeps_the_relevant_ability() {
        let (registry, ids) = registry_with(
            &[
                ("crm.clients.query", &["crm.clients"], "list of client rows"),
                (
                    "billing.invoice.send",
                    &["billing"],
                    "invoice dispatch receipt",
                ),
            ],
            60,
        );
        let config = RetrievalConfig::default();
        let outcome = shortlist_abilities(
            "query the clients list",
            &ids,
            &registry,
            &HashEmbedder::new(64).expect("embedder"),
            &config,
        );
        assert!(outcome.applied);
        assert_eq!(outcome.considered, ids.len());
        assert!(
            outcome.ability_ids.len() < ids.len(),
            "shortlist must be smaller than the catalog"
        );
        assert_eq!(
            outcome.ability_ids.first().map(String::as_str),
            Some("crm.clients.query"),
            "the relevant ability must rank first, got {:?}",
            outcome.ability_ids
        );
    }

    /// Retrieval nominates; it must not resolve. Equally-scoring abilities are all retained rather
    /// than arbitrarily cut at the target boundary.
    #[test]
    fn near_ties_are_retained_not_resolved() {
        let mut named: Vec<(String, Vec<&str>, String)> = Vec::new();
        for index in 0..8 {
            named.push((
                format!("report.export.v{index}"),
                vec!["report"],
                "export a report".to_string(),
            ));
        }
        let borrowed: Vec<(&str, &[&str], &str)> = named
            .iter()
            .map(|(id, tags, meaning)| (id.as_str(), tags.as_slice(), meaning.as_str()))
            .collect();
        let (registry, ids) = registry_with(&borrowed, 40);
        let config = RetrievalConfig {
            always_inline_below: 4,
            core_reserve: 3,
            ..Default::default()
        };
        let outcome = shortlist_abilities(
            "export a report",
            &ids,
            &registry,
            &HashEmbedder::new(64).expect("embedder"),
            &config,
        );
        let exports = outcome
            .ability_ids
            .iter()
            .filter(|id| id.starts_with("report.export."))
            .count();
        assert!(
            exports > config.core_reserve,
            "all {exports} equally-scoring exports should survive a reserve of {}",
            config.core_reserve
        );
    }

    /// The `std.*` catalog is large and uniform; a tenant's capabilities are few and are the point
    /// of the prompt. A utility tool sharing a word with the utterance must not displace them.
    #[test]
    fn utility_tools_never_displace_tenant_abilities() {
        let mut registry = Registry::default();
        let mut ids = Vec::new();
        // Three tenant abilities, none of which share vocabulary with the utterance.
        for id in ["auth.otp.send", "auth.otp.verify", "crm.clients.query"] {
            registry
                .register_tool(tool(id, &["tenant"], "tenant capability"))
                .unwrap();
            let mut contract = AbilityContract::pure(id);
            contract.tool_deps = vec![id.into()];
            registry.abilities.insert(id.into(), contract);
            ids.push(id.to_string());
        }
        // A large std catalog. The `std.data.*` entries describe lists, so they — and only they —
        // match the utterance lexically.
        for (name, meaning) in [
            ("std.data.count", "count items in a list"),
            ("std.data.first", "first item of a list"),
            ("std.data.last", "last item of a list"),
            ("std.data.take", "take n items from a list"),
            ("std.data.append", "append to a list"),
            ("std.data.filter_equals", "filter a list by field"),
            ("std.data.sort_by_field", "sort a list by field"),
            ("std.data.dedupe", "remove duplicates from a list"),
            ("std.data.map_field", "project a field from a list"),
            ("std.text.length", "character length of text"),
            ("std.text.concat", "concatenate two strings"),
            ("std.text.join", "join strings with a separator"),
            ("std.text.split", "split a string"),
            ("std.math.sum", "arithmetic total"),
            ("std.math.average", "arithmetic mean"),
            ("std.logic.and", "boolean conjunction"),
            ("std.logic.or", "boolean disjunction"),
            ("std.logic.not", "boolean negation"),
            ("std.control.coalesce", "first non-null value"),
            ("std.validate.email", "email address check"),
            ("std.validate.url", "url check"),
            ("std.validate.uuid", "uuid check"),
            ("std.validate.phone_e164", "phone number check"),
            ("std.data.keys", "keys of a map"),
            ("std.data.values", "values of a map"),
            ("std.data.merge", "merge two maps"),
        ] {
            registry
                .register_tool(tool(name, &["std"], meaning))
                .unwrap();
            let mut contract = AbilityContract::pure(name);
            contract.tool_deps = vec![name.into()];
            registry.abilities.insert(name.into(), contract);
            ids.push(name.to_string());
        }

        // "list jobs" matches the std.data.* descriptors and nothing the tenant declared.
        let outcome = shortlist_abilities(
            "list jobs",
            &ids,
            &registry,
            &HashEmbedder::new(64).expect("embedder"),
            &RetrievalConfig::default(),
        );
        assert!(outcome.applied, "catalog is large enough to shortlist");
        for tenant in ["auth.otp.send", "auth.otp.verify", "crm.clients.query"] {
            assert!(
                outcome.ability_ids.iter().any(|id| id == tenant),
                "tenant ability `{tenant}` was displaced by std tools: {:?}",
                outcome.ability_ids
            );
        }
        assert!(
            outcome.ability_ids.len() < ids.len(),
            "std catalog should still be trimmed"
        );
    }

    #[test]
    fn ordering_is_deterministic_across_runs() {
        let (registry, ids) = registry_with(
            &[("crm.clients.query", &["crm.clients"], "list of client rows")],
            60,
        );
        let embedder = HashEmbedder::new(64).expect("embedder");
        let config = RetrievalConfig::default();
        let first = shortlist_abilities("query the clients", &ids, &registry, &embedder, &config);
        let second = shortlist_abilities("query the clients", &ids, &registry, &embedder, &config);
        assert_eq!(first.ability_ids, second.ability_ids);
    }

    /// When nothing scores, retrieval must hand back the whole catalog. A prompt that is too long
    /// degrades; one missing the only viable ability fails outright.
    #[test]
    fn no_signal_falls_back_to_the_whole_catalog() {
        struct BlindEmbedder;
        impl Embedder for BlindEmbedder {
            fn dimension(&self) -> usize {
                4
            }
            fn embed(&self, _text: &str) -> AelioResult<Vec<f32>> {
                Ok(vec![0.0; 4])
            }
        }

        let (registry, ids) = registry_with(&[("crm.clients.query", &["crm"], "client rows")], 60);
        let outcome = shortlist_abilities(
            "zzzz qqqq xxxx",
            &ids,
            &registry,
            &BlindEmbedder,
            &RetrievalConfig::default(),
        );
        assert!(!outcome.applied);
        assert_eq!(outcome.ability_ids, ids);
    }

    /// A real embedding model must be able to bridge vocabulary the tenant never declared.
    #[test]
    fn semantic_embedder_bridges_undeclared_vocabulary() {
        struct CustomerEmbedder;
        impl Embedder for CustomerEmbedder {
            fn dimension(&self) -> usize {
                3
            }
            fn embed(&self, text: &str) -> AelioResult<Vec<f32>> {
                // "customer" and "client" occupy the same axis; billing is orthogonal.
                let v = if text.contains("client") || text.contains("customer") {
                    vec![1.0, 0.0, 0.0]
                } else if text.contains("invoice") || text.contains("billing") {
                    vec![0.0, 1.0, 0.0]
                } else {
                    vec![0.0, 0.0, 1.0]
                };
                Ok(v)
            }
            fn supports_semantic_equivalence(&self) -> bool {
                true
            }
        }

        let (registry, ids) = registry_with(
            &[
                ("crm.clients.query", &["crm.clients"], "list of client rows"),
                ("billing.invoice.send", &["billing"], "invoice dispatch"),
            ],
            60,
        );
        // "customer" appears nowhere in the catalog — only a semantic model can bridge it.
        let outcome = shortlist_abilities(
            "pull up that customer",
            &ids,
            &registry,
            &CustomerEmbedder,
            &RetrievalConfig::default(),
        );
        assert!(outcome.applied);
        assert_eq!(
            outcome.ability_ids.first().map(String::as_str),
            Some("crm.clients.query"),
            "semantic bridge should surface the clients ability, got {:?}",
            outcome.ability_ids
        );
    }
}
