//! Learn.* — situation keys, tier ladder, typecheck, propose/promote.

use crate::abilities::registry::{
    ProcedureEvidence, ProcedureProvenance, ProcedureSpec, ProcedureStatus, Registry,
    SituationFilter,
};
use crate::contract::{AbilityContract, AbilityPath, Predicate};
use crate::embedding::{Embedder, HashEmbedder};
use crate::provider::{
    ClosedOutputSpec, ClosedOutputType, LlmProvider, LlmRequest, PromptSlotSpec, PromptSpec,
};
use crate::tenant::ParamSource;
use crate::types::{AelioError, AelioResult, LookupTier, ReasonCode, Sensitivity};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Default dimension for in-process situation vectors (tier-1 kNN).
pub const SITUATION_EMBED_DIM: usize = 32;

/// Tier-1 accepts a near hit only when (top − second) clears this margin.
pub const TIER1_MARGIN_THRESHOLD: f64 = 0.8;

/// Bucketed fields only — raw precision kills tier-0 hit rate.
/// Personality MUST NOT enter σ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SituationKey {
    pub state: String,
    pub intent_class: String,
    pub slots_filled: Vec<String>,
    pub reachable_caps: Vec<String>,
    pub flow_ctx: String,
    pub turn_index_bucket: TurnBucket,
    pub last_seen_bucket: SeenBucket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnBucket {
    First,
    Early,
    Established,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeenBucket {
    Never,
    Recent,
    Lapsed,
    Dormant,
}

pub fn bucket_turn_index(turn_index: u64) -> TurnBucket {
    match turn_index {
        0 => TurnBucket::First,
        1..=5 => TurnBucket::Early,
        _ => TurnBucket::Established,
    }
}

/// `last_seen_secs_ago: None` = never
pub fn bucket_last_seen(last_seen_secs_ago: Option<u64>) -> SeenBucket {
    match last_seen_secs_ago {
        None => SeenBucket::Never,
        Some(s) if s < 3600 * 24 => SeenBucket::Recent,
        Some(s) if s < 3600 * 24 * 14 => SeenBucket::Lapsed,
        Some(_) => SeenBucket::Dormant,
    }
}

pub fn situation_key(
    state: &str,
    intent_class: &str,
    slots_filled: Vec<String>,
    reachable_caps: Vec<String>,
    flow_ctx: Option<&str>,
    turn_index: u64,
    last_seen_secs_ago: Option<u64>,
) -> SituationKey {
    let mut slots = slots_filled;
    slots.sort();
    let mut caps = reachable_caps;
    caps.sort();
    SituationKey {
        state: state.into(),
        intent_class: intent_class.into(),
        slots_filled: slots,
        reachable_caps: caps,
        flow_ctx: flow_ctx.unwrap_or("none").into(),
        turn_index_bucket: bucket_turn_index(turn_index),
        last_seen_bucket: bucket_last_seen(last_seen_secs_ago),
    }
}

pub fn situation_hash(sigma: &SituationKey) -> String {
    let bytes = serde_json::to_vec(sigma).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    hex::encode(&digest[..16])
}

/// Near-situation text for tier-1 embeddings.
///
/// Excludes `turn_index_bucket` and `last_seen_bucket` so first vs returning visits can still
/// retrieve the same procedure when state/intent/slots/caps/flow align.
///
/// Intent/state are repeated so bag-of-hash embedders (and real models) weight the
/// discriminative axes; shared template tokens alone must not clear the 0.8 margin.
pub fn situation_near_text(sigma: &SituationKey) -> String {
    format_near_text(
        &sigma.intent_class,
        &sigma.state,
        &sigma.slots_filled,
        &sigma.reachable_caps,
        &sigma.flow_ctx,
    )
}

pub fn situation_filter_near_text(filter: &SituationFilter) -> String {
    let mut slots = filter.required_slots.clone();
    slots.sort();
    let mut caps = filter.capability_tags.clone();
    caps.sort();
    format_near_text(
        filter.intent_class.as_deref().unwrap_or(""),
        filter.state.as_deref().unwrap_or(""),
        &slots,
        &caps,
        "none",
    )
}

fn format_near_text(
    intent: &str,
    state: &str,
    slots: &[String],
    caps: &[String],
    flow: &str,
) -> String {
    // Repeat intent/state so cosine on HashEmbedder separates unrelated situations.
    format!(
        "intent={i} intent={i} intent={i} intent={i} state={s} state={s} slots={slots} caps={caps} flow={flow}",
        i = intent,
        s = state,
        slots = slots.join(","),
        caps = caps.join(","),
        flow = flow,
    )
}

pub fn default_situation_embedder() -> HashEmbedder {
    HashEmbedder::new(SITUATION_EMBED_DIM).expect("SITUATION_EMBED_DIM is positive")
}

pub fn embed_situation(embedder: &dyn Embedder, sigma: &SituationKey) -> AelioResult<Vec<f32>> {
    embedder.embed(&situation_near_text(sigma))
}

pub fn embed_situation_filter(
    embedder: &dyn Embedder,
    filter: &SituationFilter,
) -> AelioResult<Vec<f32>> {
    embedder.embed(&situation_filter_near_text(filter))
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = f64::from(*x);
        let yf = f64::from(*y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom <= f64::EPSILON {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierLookup {
    pub tier: LookupTier,
    pub procedure_id: Option<String>,
    pub path: Option<AbilityPath>,
    pub margin: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalSpec {
    pub intent: String,
    pub produces: Vec<String>,
    pub available_slots: Vec<String>,
    pub allowed_capabilities: Vec<String>,
    pub allow_effects: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CompositionBudget {
    pub max_procedures: usize,
    pub max_steps: usize,
}

impl Default for CompositionBudget {
    fn default() -> Self {
        Self {
            max_procedures: 8,
            max_steps: 32,
        }
    }
}

pub fn lookup_tier(reg: &Registry, sigma: &SituationKey) -> TierLookup {
    lookup_tier_with_embedder(reg, sigma, &default_situation_embedder())
}

/// Tier ladder:
/// - Tier0: exact `situation_hash` hit
/// - Tier1: scored vector kNN over near-situation embeddings; accept only if margin ≥ 0.8
/// - otherwise Tier3 (caller may still attempt Tier2 compose)
pub fn lookup_tier_with_embedder(
    reg: &Registry,
    sigma: &SituationKey,
    embedder: &dyn Embedder,
) -> TierLookup {
    let hash = situation_hash(sigma);
    if let Some(proc) = reg.lookup_procedure_by_hash(&hash) {
        return TierLookup {
            tier: LookupTier::Tier0,
            procedure_id: Some(proc.id.clone()),
            path: Some(proc.path.clone()),
            margin: 1.0,
        };
    }

    let Ok(query) = embed_situation(embedder, sigma) else {
        return TierLookup {
            tier: LookupTier::Tier3,
            procedure_id: None,
            path: None,
            margin: 0.0,
        };
    };

    let mut scored: Vec<(&ProcedureSpec, f64)> = reg
        .procedures
        .values()
        .filter(|proc| {
            proc.status == ProcedureStatus::Promoted
                && !proc.situation_embedding.is_empty()
                && proc
                    .situation_filter
                    .state
                    .as_ref()
                    .is_none_or(|state| state == &sigma.state)
                && proc
                    .situation_filter
                    .required_slots
                    .iter()
                    .all(|slot| sigma.slots_filled.contains(slot))
                && proc
                    .situation_filter
                    .capability_tags
                    .iter()
                    .all(|required| {
                        sigma.reachable_caps.iter().any(|available| {
                            required == available
                                || (available.ends_with('*')
                                    && required.starts_with(
                                        available.trim_end_matches('*').trim_end_matches('.'),
                                    ))
                        })
                    })
        })
        .map(|proc| (proc, cosine_similarity(&query, &proc.situation_embedding)))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let top = scored.first().copied();
    let second = scored.get(1).map(|(_, score)| *score).unwrap_or(0.0);
    if let Some((proc, top_score)) = top {
        // Margin = top − second. With one candidate, second is 0 → absolute score must clear 0.8.
        let margin = top_score - second;
        if top_score >= TIER1_MARGIN_THRESHOLD && margin >= TIER1_MARGIN_THRESHOLD {
            return TierLookup {
                tier: LookupTier::Tier1,
                procedure_id: Some(proc.id.clone()),
                path: Some(proc.path.clone()),
                margin,
            };
        }
        return TierLookup {
            tier: LookupTier::Tier3,
            procedure_id: None,
            path: None,
            margin,
        };
    }

    TierLookup {
        tier: LookupTier::Tier3,
        procedure_id: None,
        path: None,
        margin: 0.0,
    }
}

/// Evidence-directed bounded planning over promoted procedures.
///
/// A procedure is eligible when all of its declared requirements are already available. Each
/// selected procedure adds its declared postcondition evidence to the planning state. This is a
/// forward-chaining search rather than the old one-level "find one producer for each slot" lookup,
/// so an arbitrary chain A→B→C is reachable without a vertical-specific goal name.
pub fn compose(reg: &Registry, goal: &GoalSpec, budget: CompositionBudget) -> Option<AbilityPath> {
    if goal.produces.is_empty() || budget.max_procedures == 0 || budget.max_steps == 0 {
        return None;
    }
    let mut procedures: Vec<&ProcedureSpec> = reg
        .procedures
        .values()
        .filter(|p| {
            p.status == ProcedureStatus::Promoted
                && (goal.allow_effects || !p.contract.effectful)
                // A path learned for one customer intent is not evidence that it solves another.
                // Without this guard, generic postconditions can compose an unrelated warm path.
                && p
                    .situation_filter
                    .intent_class
                    .as_ref()
                    .is_none_or(|intent| intent == &goal.intent)
                && p.situation_filter.capability_tags.iter().all(|required| {
                    goal.allowed_capabilities.iter().any(|allowed| {
                        required == allowed
                            || (allowed.ends_with('*')
                                && required.starts_with(
                                    allowed.trim_end_matches('*').trim_end_matches('.'),
                                ))
                    })
                })
        })
        .collect();
    procedures.sort_by(|left, right| {
        right
            .evidence
            .success_rate
            .total_cmp(&left.evidence.success_rate)
            .then(right.evidence.observations.cmp(&left.evidence.observations))
            .then(left.id.cmp(&right.id))
    });

    struct SearchState {
        known: std::collections::BTreeSet<String>,
        selected: std::collections::BTreeSet<String>,
        visiting: std::collections::BTreeSet<String>,
        path_steps: Vec<crate::contract::PathStep>,
    }

    fn satisfy(
        target: &str,
        procedures: &[&ProcedureSpec],
        state: &mut SearchState,
        budget: CompositionBudget,
    ) -> bool {
        if state.known.contains(target) {
            return true;
        }
        if !state.visiting.insert(target.to_string()) {
            return false;
        }
        for procedure in procedures {
            if state.selected.contains(&procedure.id)
                || !procedure
                    .contract
                    .postconditions
                    .iter()
                    .any(|post| matches_produces(post, target))
            {
                continue;
            }
            let snapshot = (
                state.known.clone(),
                state.selected.clone(),
                state.path_steps.clone(),
            );
            let requirements_met = procedure
                .situation_filter
                .required_slots
                .iter()
                .all(|required| satisfy(required, procedures, state, budget));
            if requirements_met
                && state.selected.len() < budget.max_procedures
                && state
                    .path_steps
                    .len()
                    .saturating_add(procedure.path.steps.len())
                    <= budget.max_steps
            {
                state.selected.insert(procedure.id.clone());
                state.path_steps.extend(procedure.path.steps.clone());
                for postcondition in &procedure.contract.postconditions {
                    state.known.extend(produced_names(postcondition));
                }
                state.visiting.remove(target);
                return state.known.contains(target);
            }
            state.known = snapshot.0;
            state.selected = snapshot.1;
            state.path_steps = snapshot.2;
        }
        state.visiting.remove(target);
        false
    }

    let mut state = SearchState {
        known: goal.available_slots.iter().cloned().collect(),
        selected: std::collections::BTreeSet::new(),
        visiting: std::collections::BTreeSet::new(),
        path_steps: Vec::new(),
    };
    for target in &goal.produces {
        if !satisfy(target, &procedures, &mut state, budget) {
            return None;
        }
    }
    let path = AbilityPath {
        steps: state.path_steps,
    };
    (!path.is_empty() && typecheck(reg, &path).is_ok()).then_some(path)
}

fn matches_produces(post: &Predicate, name: &str) -> bool {
    match post {
        Predicate::Present { path } => path == name || path.ends_with(name),
        Predicate::Eq { path, .. } => path == name,
        _ => false,
    }
}

fn produced_names(predicate: &Predicate) -> Vec<String> {
    match predicate {
        Predicate::Present { path } | Predicate::Eq { path, .. } => vec![path.clone()],
        Predicate::And { of } => of.iter().flat_map(produced_names).collect(),
        _ => Vec::new(),
    }
}

/// Evidence a learned path needs before it can run without asking the user.
pub fn path_required_evidence(reg: &Registry, path: &AbilityPath) -> AelioResult<Vec<String>> {
    let mut required = std::collections::BTreeSet::new();
    for step in &path.steps {
        let tools = resolved_step_tools(reg, &step.ability_id)?;
        for tool in tools {
            for parameter in &tool.params {
                if !parameter.required || parameter.default.is_some() {
                    continue;
                }
                match &parameter.source {
                    ParamSource::User => {
                        required.insert(parameter.name.clone());
                    }
                    ParamSource::Slot { name } => {
                        required.insert(name.clone());
                    }
                    ParamSource::ToolOutput { ref_path } => {
                        required.insert(ref_path.clone());
                    }
                    ParamSource::Derived { .. } => {
                        required.extend(parameter.depends_on.iter().cloned());
                    }
                    ParamSource::State { .. }
                    | ParamSource::Env { .. }
                    | ParamSource::Const { .. } => {}
                }
            }
        }
    }
    Ok(required.into_iter().collect())
}

/// Verified evidence names a path can produce, derived from declared tool output semantics.
pub fn path_produced_evidence(reg: &Registry, path: &AbilityPath) -> AelioResult<Vec<String>> {
    let mut produced = std::collections::BTreeSet::new();
    for step in &path.steps {
        for tool in resolved_step_tools(reg, &step.ability_id)? {
            for (name, field) in &tool.output_semantics.fields {
                produced.insert(name.clone());
                produced.insert(field.path.clone());
            }
        }
        if let Some(ability) = reg.abilities.get(&step.ability_id) {
            for postcondition in &ability.postconditions {
                produced.extend(produced_names(postcondition));
            }
        }
    }
    Ok(produced
        .into_iter()
        .filter(|name| !name.is_empty())
        .collect())
}

/// Exact capability surface used by a path; reachable-but-unused permissions are not dependencies.
pub fn path_capabilities(reg: &Registry, path: &AbilityPath) -> AelioResult<Vec<String>> {
    let mut capabilities = std::collections::BTreeSet::new();
    for step in &path.steps {
        for tool in resolved_step_tools(reg, &step.ability_id)? {
            capabilities.extend(tool.capability_tags.iter().cloned());
        }
    }
    Ok(capabilities.into_iter().collect())
}

fn resolved_step_tools<'a>(
    reg: &'a Registry,
    step_id: &str,
) -> AelioResult<Vec<&'a crate::tenant::ToolSpec>> {
    if let Some(tool) = reg.tools.get(step_id) {
        return Ok(vec![tool]);
    }
    let tools = reg.lookup_tool_by_capability(step_id);
    if tools.len() > 1 {
        return Err(AelioError::new(
            ReasonCode::Ambiguous,
            format!("capability {step_id} resolves to multiple tools"),
        ));
    }
    Ok(tools)
}

/// Static typecheck: every step exists and adjacent declared schemas unify.
pub fn typecheck(reg: &Registry, path: &AbilityPath) -> AelioResult<()> {
    if path.steps.is_empty() {
        return Err(AelioError::new(ReasonCode::Unsatisfiable, "empty path"));
    }
    for step in &path.steps {
        if !reg.abilities.contains_key(&step.ability_id)
            && !reg.tools.contains_key(&step.ability_id)
            && reg.lookup_tool_by_capability(&step.ability_id).is_empty()
            && !is_builtin(&step.ability_id)
        {
            return Err(AelioError::new(
                ReasonCode::Unsatisfiable,
                format!("unknown ability {}", step.ability_id),
            ));
        }
    }
    for pair in path.steps.windows(2) {
        let Some(left) = reg.abilities.get(&pair[0].ability_id) else {
            continue;
        };
        let Some(right) = reg.abilities.get(&pair[1].ability_id) else {
            continue;
        };
        if !right.input.accepts_output_of(&left.output) {
            return Err(AelioError::new(
                ReasonCode::Unsatisfiable,
                format!("{} output does not satisfy {} input", left.id, right.id),
            ));
        }
    }
    Ok(())
}

fn is_builtin(id: &str) -> bool {
    matches!(
        id,
        "State.Direction"
            | "Registry.Capabilities"
            | "Express.Template"
            | "Express.Synthesize"
            | "Express.Ask"
            | "Sense.Env"
            | "Sense.Session"
            | "State.Read"
            | "Bind.ResolveAll"
            | "Invoke.Call"
            | "Judge.Confidence"
            | "Understand.Extract"
            | "Sum"
            | "Learn.ProposePath"
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub situation_hash: String,
    /// The full situation key, retained so promotion can embed the *real* σ (not an empty filter)
    /// — this is what makes Tier-1 near-match work after a proposal is promoted.
    #[serde(default)]
    pub sigma: Option<SituationKey>,
    pub path: AbilityPath,
    pub observations: u64,
    pub successes: u64,
    pub effectful: bool,
    pub status: ProcedureStatus,
}

pub type ProposalMap = IndexMap<String, Proposal>;

/// Stable identity for one (situation, path) alternative. Keeping the path digest in the key is
/// what allows runner-up procedures to accumulate evidence instead of each new proposal silently
/// overwriting the previous path for the same situation.
pub fn proposal_id(sigma: &SituationKey, path: &AbilityPath) -> String {
    let path_bytes = serde_json::to_vec(path).unwrap_or_default();
    let path_hash = hex::encode(&Sha256::digest(path_bytes)[..6]);
    format!("prop_{}_{}", &situation_hash(sigma)[..8], path_hash)
}

/// Default asymmetric-gate thresholds for in-session promotion. Slow to promote (needs repeated
/// clean observations of the *same* σ), instant to suspend elsewhere.
pub const PROMOTE_MIN_OBSERVATIONS: u64 = 3;
pub const PROMOTE_MIN_SUCCESS_RATE: f64 = 0.8;

/// Outcome of observing a turn and attempting promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveOutcome {
    /// Evidence recorded; the gate is not yet met.
    Accumulating { observations: u64 },
    /// The proposal cleared the gate and is now a promoted procedure in the registry.
    Promoted { procedure_id: String },
}

pub fn propose_path_prompt_spec() -> PromptSpec {
    PromptSpec {
        id: "aelio.propose_path".into(),
        version: "1".into(),
        instruction: concat!(
            "Select an ordered path using only the declared ability identifiers. ",
            "Each step has a declared ability_id and an args object. Fill args only from ",
            "the customer request; never invent identifiers or facts. Return exactly one JSON object."
        )
        .into(),
        slots: vec![
            PromptSlotSpec {
                name: "situation".into(),
                required: true,
                sensitivity: Sensitivity::Pii,
            },
            PromptSlotSpec {
                name: "abilities".into(),
                required: true,
                sensitivity: Sensitivity::None,
            },
            PromptSlotSpec {
                name: "request".into(),
                required: true,
                sensitivity: Sensitivity::Pii,
            },
        ],
        max_chars: 16_000,
        max_tokens: 4_000,
        truncation_order: vec!["situation".into()],
        output: ClosedOutputSpec {
            required_fields: vec!["steps".into()],
            allowed_fields: vec!["steps".into()],
            field_types: indexmap::indexmap! {
                "steps".into() => ClosedOutputType::PathStepArray,
            },
        },
    }
}

pub fn propose_path_with_provider(
    sigma: &SituationKey,
    request: &str,
    ability_ids: &[String],
    ability_declarations: &[String],
    provider: &mut dyn LlmProvider,
) -> AelioResult<(AbilityPath, String)> {
    let spec = propose_path_prompt_spec();
    let bindings = indexmap::indexmap! {
        "situation".into() => serde_json::to_string(sigma)
            .map_err(|error| AelioError::new(ReasonCode::Validation, error.to_string()))?,
        "abilities".into() => ability_declarations.join("\n"),
        "request".into() => request.into(),
    };
    let prompt_hash = spec.canonical_hash()?;
    let response = provider.complete(&LlmRequest {
        prompt: spec.render(&bindings)?,
        model: "default".into(),
        temperature: 0.0,
    })?;
    let parsed = spec.parse_output(&response.content)?;
    let steps = parsed
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            AelioError::new(
                ReasonCode::ParseError,
                "provider output field `steps` must be an array",
            )
        })?
        .iter()
        .map(|step| {
            serde_json::from_value::<crate::contract::PathStep>(step.clone()).map_err(|error| {
                AelioError::new(
                    ReasonCode::ParseError,
                    format!("provider output path step is invalid: {error}"),
                )
            })
        })
        .collect::<AelioResult<Vec<_>>>()?;
    if steps.is_empty() {
        return Err(AelioError::new(
            ReasonCode::ParseError,
            "provider output path cannot be empty",
        ));
    }
    if let Some(unknown) = steps.iter().find(|step| {
        !ability_ids
            .iter()
            .any(|ability| ability == &step.ability_id)
    }) {
        return Err(AelioError::new(
            ReasonCode::ParseError,
            format!(
                "provider selected undeclared ability `{}`",
                unknown.ability_id
            ),
        ));
    }
    Ok((AbilityPath { steps }, prompt_hash))
}

pub fn propose(
    proposals: &mut ProposalMap,
    sigma: &SituationKey,
    path: AbilityPath,
    effectful: bool,
) -> String {
    let id = proposal_id(sigma, &path);
    proposals.insert(
        id.clone(),
        Proposal {
            id: id.clone(),
            situation_hash: situation_hash(sigma),
            sigma: Some(sigma.clone()),
            path,
            observations: 0,
            successes: 0,
            effectful,
            status: ProcedureStatus::Proposed,
        },
    );
    id
}

/// Record one behavioral observation for σ, accumulating across turns (never resetting). The first
/// observation of a σ creates the proposal; repeats increment its evidence. This is what lets the
/// same situation, seen enough times, cross the promotion gate instead of forever re-proposing.
pub fn observe(
    proposals: &mut ProposalMap,
    sigma: &SituationKey,
    path: AbilityPath,
    effectful: bool,
    success: bool,
) -> String {
    let id = proposal_id(sigma, &path);
    let entry = proposals.entry(id.clone()).or_insert_with(|| Proposal {
        id: id.clone(),
        situation_hash: situation_hash(sigma),
        sigma: Some(sigma.clone()),
        path: path.clone(),
        observations: 0,
        successes: 0,
        effectful,
        status: ProcedureStatus::Proposed,
    });
    if entry.status != ProcedureStatus::Promoted {
        entry.observations += 1;
        if success {
            entry.successes += 1;
        }
        // Keep the latest successful path and σ so promotion uses current shapes.
        entry.path = path;
        entry.sigma = Some(sigma.clone());
    }
    id
}

/// Observe a turn's outcome and promote the σ's proposal into the live registry once the asymmetric
/// gate clears. Effectful paths never auto-promote (they require tenant approval, decision P5), so
/// only deterministic read/express paths warm up on their own. On promotion the procedure carries
/// the real σ hash + embedding, so the next identical σ hits Tier-0 and near σ hit Tier-1.
#[allow(clippy::too_many_arguments)]
pub fn observe_and_promote(
    proposals: &mut ProposalMap,
    reg: &mut Registry,
    sigma: &SituationKey,
    path: AbilityPath,
    effectful: bool,
    success: bool,
    tenant_approved: bool,
    embedder: &dyn Embedder,
) -> ObserveOutcome {
    let id = observe(proposals, sigma, path, effectful, success);
    let proposal = proposals
        .get(&id)
        .cloned()
        .expect("observe just inserted the proposal");
    if proposal.status == ProcedureStatus::Promoted {
        return ObserveOutcome::Promoted { procedure_id: id };
    }
    match promote(
        reg,
        &proposal,
        PROMOTE_MIN_OBSERVATIONS,
        PROMOTE_MIN_SUCCESS_RATE,
        tenant_approved,
        embedder,
    ) {
        Ok(spec) => {
            let procedure_id = spec.id.clone();
            reg.register_procedure(spec);
            if let Some(entry) = proposals.get_mut(&id) {
                entry.status = ProcedureStatus::Promoted;
            }
            ObserveOutcome::Promoted { procedure_id }
        }
        Err(_) => ObserveOutcome::Accumulating {
            observations: proposal.observations,
        },
    }
}

/// A path is effectful if any step resolves to a tool or capability tag — i.e. it can act on the
/// tenant's world. Such paths are gated behind tenant approval before they may promote.
pub fn path_is_effectful(reg: &Registry, path: &AbilityPath) -> bool {
    path.steps.iter().any(|step| {
        reg.tools
            .get(&step.ability_id)
            .is_some_and(|tool| tool.effectful)
            || reg
                .lookup_tool_by_capability(&step.ability_id)
                .iter()
                .any(|tool| tool.effectful)
            || reg
                .abilities
                .get(&step.ability_id)
                .is_some_and(|ability| ability.effectful)
    })
}

/// Asymmetric gate: slow to promote, instant to suspend.
///
/// `embedder` must be the *same* embedder the hot loop queries Tier-1 with (the runtime's configured
/// one — bag-of-hash offline, or the gateway-served model), so a promoted procedure's σ embedding
/// lives in the same space as the queries that will look it up.
pub fn promote(
    reg: &Registry,
    proposal: &Proposal,
    min_observations: u64,
    min_success_rate: f64,
    tenant_approved: bool,
    embedder: &dyn Embedder,
) -> AelioResult<ProcedureSpec> {
    typecheck(reg, &proposal.path)?;
    if proposal.observations < min_observations {
        return Err(AelioError::new(
            ReasonCode::GateNotMet,
            format!(
                "need {min_observations} observations, have {}",
                proposal.observations
            ),
        ));
    }
    let rate = if proposal.observations == 0 {
        0.0
    } else {
        proposal.successes as f64 / proposal.observations as f64
    };
    if rate < min_success_rate {
        return Err(AelioError::new(
            ReasonCode::GateNotMet,
            format!("success rate {rate} < {min_success_rate}"),
        ));
    }
    if proposal.effectful && !tenant_approved {
        return Err(AelioError::new(
            ReasonCode::GateNotMet,
            "effectful procedure requires tenant approval",
        ));
    }
    let tool_deps = path_tool_dependencies(reg, &proposal.path)?;
    let prompt_deps = path_prompt_dependencies(reg, &proposal.path);
    let mut contract = AbilityContract::pure(proposal.id.clone());
    contract.tool_deps = tool_deps.clone();

    let required_evidence = path_required_evidence(reg, &proposal.path)?;
    let used_capabilities = path_capabilities(reg, &proposal.path)?;
    let mut produced_evidence = path_produced_evidence(reg, &proposal.path)?;

    // Derive the filter, embedding, and evidence contract from the executable path and the *real*
    // σ. Reachable permissions are not path dependencies, and slots that happened to be filled in
    // one observed turn are not necessarily requirements. This distinction is what makes generic
    // multi-hop planning sound.
    let (situation_filter, situation_embedding, postconditions) = match &proposal.sigma {
        Some(sigma) => {
            let filter = SituationFilter {
                state: Some(sigma.state.clone()),
                intent_class: Some(sigma.intent_class.clone()),
                required_slots: required_evidence,
                capability_tags: used_capabilities,
            };
            let embedding = embed_situation(embedder, sigma).unwrap_or_default();
            produced_evidence.push(sigma.intent_class.clone());
            produced_evidence.sort();
            produced_evidence.dedup();
            let posts = produced_evidence
                .into_iter()
                .map(|path| Predicate::Present { path })
                .collect();
            (filter, embedding, posts)
        }
        None => {
            let filter = SituationFilter::default();
            let embedding = embed_situation_filter(embedder, &filter).unwrap_or_default();
            (filter, embedding, Vec::new())
        }
    };
    contract.postconditions = postconditions;
    Ok(ProcedureSpec {
        id: proposal.id.clone(),
        version: "1".into(),
        tenant_id: String::new(),
        situation_hash: proposal.situation_hash.clone(),
        situation_filter,
        situation_embedding,
        path: proposal.path.clone(),
        contract,
        tool_deps,
        prompt_deps,
        evidence: ProcedureEvidence {
            observations: proposal.observations,
            success_rate: rate,
            mean_cost: 0.0,
            mean_latency_ms: 0.0,
        },
        status: ProcedureStatus::Promoted,
        provenance: ProcedureProvenance {
            origin: "tier3".into(),
            proposed_by: "system".into(),
            approved_by: if tenant_approved {
                Some("tenant".into())
            } else {
                None
            },
        },
        supersedes: None,
    })
}

pub fn path_prompt_dependencies(reg: &Registry, path: &AbilityPath) -> Vec<String> {
    let mut dependencies = std::collections::BTreeSet::new();
    for step in &path.steps {
        if let Some(hash) = reg
            .abilities
            .get(&step.ability_id)
            .and_then(|ability| ability.prompt_hash.clone())
        {
            dependencies.insert(hash);
        }
    }
    dependencies.into_iter().collect()
}

/// Stable fingerprint of every prompt-backed ability in an executable path. The proposal prompt
/// is intentionally excluded once the path is frozen; only prompts that can affect execution are
/// runtime dependencies of the immutable procedure.
pub fn path_prompt_dependency_fingerprint(reg: &Registry, path: &AbilityPath) -> String {
    use sha2::{Digest, Sha256};

    let mut dependencies = path
        .steps
        .iter()
        .filter_map(|step| {
            reg.abilities
                .get(&step.ability_id)
                .and_then(|ability| ability.prompt_hash.as_ref())
                .map(|hash| (step.ability_id.as_str(), hash.as_str()))
        })
        .collect::<Vec<_>>();
    dependencies.sort_unstable();
    if dependencies.is_empty() {
        return "none".into();
    }
    let mut hasher = Sha256::new();
    for (ability, hash) in dependencies {
        hasher.update((ability.len() as u64).to_le_bytes());
        hasher.update(ability.as_bytes());
        hasher.update((hash.len() as u64).to_le_bytes());
        hasher.update(hash.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Resolve the complete transitive tool dependency set for an immutable procedure version.
/// Direct tool ids, capability references, and ability-declared dependencies all participate.
pub fn path_tool_dependencies(reg: &Registry, path: &AbilityPath) -> AelioResult<Vec<String>> {
    let mut dependencies = std::collections::BTreeSet::new();
    for step in &path.steps {
        if reg.tools.contains_key(&step.ability_id) {
            dependencies.insert(step.ability_id.clone());
        }
        let capability_tools = reg.lookup_tool_by_capability(&step.ability_id);
        if capability_tools.len() > 1 {
            return Err(AelioError::new(
                ReasonCode::Ambiguous,
                format!("capability {} resolves to multiple tools", step.ability_id),
            ));
        }
        dependencies.extend(capability_tools.into_iter().map(|tool| tool.id.clone()));
        if let Some(ability) = reg.abilities.get(&step.ability_id) {
            dependencies.extend(ability.tool_deps.iter().cloned());
        }
    }
    Ok(dependencies.into_iter().collect())
}

pub fn demote(reg: &mut Registry, procedure_id: &str, _reason: &str) -> AelioResult<()> {
    let p = reg
        .procedures
        .get_mut(procedure_id)
        .ok_or_else(|| AelioError::new(ReasonCode::NotFound, procedure_id))?;
    p.status = ProcedureStatus::Suspended;
    Ok(())
}

/// Per-step credit assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepCredit {
    pub step_id: String,
    pub credit: f64,
}

pub fn attribute(
    step_ids: &[String],
    failed_step: Option<&str>,
    outcome_score: f64,
) -> Vec<StepCredit> {
    step_ids
        .iter()
        .map(|id| {
            let credit = if Some(id.as_str()) == failed_step {
                -1.0
            } else {
                outcome_score
            };
            StepCredit {
                step_id: id.clone(),
                credit,
            }
        })
        .collect()
}

/// Cold-path proposal: constrained ordered path over declared abilities, never prose.
pub fn propose_path_greeting() -> AbilityPath {
    AbilityPath::seq([
        "State.Direction",
        "Registry.Capabilities",
        "Express.Synthesize",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucketing_collapses_turn_index() {
        let a = situation_key("unauth", "greeting", vec![], vec![], None, 0, None);
        let b = situation_key("unauth", "greeting", vec![], vec![], None, 0, None);
        assert_eq!(situation_hash(&a), situation_hash(&b));
        // turn 0 and turn 1 differ by bucket
        let c = situation_key("unauth", "greeting", vec![], vec![], None, 1, None);
        assert_ne!(situation_hash(&a), situation_hash(&c));
    }

    #[test]
    fn proposal_identity_preserves_runner_up_paths_for_the_same_situation() {
        let sigma = situation_key("s", "intent", vec![], vec![], None, 0, None);
        let first = AbilityPath::seq(["Express.Template"]);
        let second = AbilityPath::seq(["Registry.Capabilities"]);
        assert_eq!(proposal_id(&sigma, &first), proposal_id(&sigma, &first));
        assert_ne!(proposal_id(&sigma, &first), proposal_id(&sigma, &second));
    }

    #[test]
    fn composition_recursively_builds_a_three_hop_evidence_chain() {
        fn procedure(id: &str, step: &str, required: &[&str], produced: &str) -> ProcedureSpec {
            ProcedureSpec {
                id: id.into(),
                version: "1".into(),
                tenant_id: "t".into(),
                situation_hash: id.into(),
                situation_filter: SituationFilter {
                    required_slots: required.iter().map(|value| (*value).into()).collect(),
                    ..Default::default()
                },
                situation_embedding: vec![],
                path: AbilityPath::seq([step]),
                contract: AbilityContract::pure(id).with_postconditions(vec![Predicate::Present {
                    path: produced.into(),
                }]),
                tool_deps: vec![],
                prompt_deps: vec![],
                evidence: ProcedureEvidence {
                    observations: 10,
                    success_rate: 1.0,
                    ..Default::default()
                },
                status: ProcedureStatus::Promoted,
                provenance: ProcedureProvenance {
                    origin: "test".into(),
                    proposed_by: "test".into(),
                    approved_by: None,
                },
                supersedes: None,
            }
        }

        let mut registry = Registry::default();
        for id in ["Sense.Env", "State.Read", "Express.Ask"] {
            registry.register_ability(AbilityContract::pure(id));
        }
        registry.register_procedure(procedure("p-a", "Sense.Env", &["name"], "customer_id"));
        registry.register_procedure(procedure(
            "p-b",
            "State.Read",
            &["customer_id"],
            "account_id",
        ));
        registry.register_procedure(procedure(
            "p-c",
            "Express.Ask",
            &["account_id"],
            "invoice_list",
        ));

        let path = compose(
            &registry,
            &GoalSpec {
                intent: "invoice.read".into(),
                produces: vec!["invoice_list".into()],
                available_slots: vec!["name".into()],
                allowed_capabilities: vec![],
                allow_effects: false,
            },
            CompositionBudget::default(),
        )
        .expect("the evidence graph is satisfiable");
        assert_eq!(
            path.steps
                .iter()
                .map(|step| step.ability_id.as_str())
                .collect::<Vec<_>>(),
            ["Sense.Env", "State.Read", "Express.Ask"]
        );
    }

    #[test]
    fn composition_never_reuses_a_path_learned_for_another_intent() {
        let mut registry = Registry::default();
        registry.register_ability(AbilityContract::pure("Express.Template"));
        registry.register_procedure(ProcedureSpec {
            id: "cancel-path".into(),
            version: "1".into(),
            tenant_id: "t".into(),
            situation_hash: "cancel-situation".into(),
            situation_filter: SituationFilter {
                intent_class: Some("cancel_order".into()),
                ..Default::default()
            },
            situation_embedding: vec![],
            path: AbilityPath::seq(["Express.Template"]),
            contract: AbilityContract::pure("cancel-path").with_postconditions(vec![
                Predicate::Present {
                    path: "general_deep".into(),
                },
            ]),
            tool_deps: vec![],
            prompt_deps: vec![],
            evidence: ProcedureEvidence {
                observations: 10,
                success_rate: 1.0,
                ..Default::default()
            },
            status: ProcedureStatus::Promoted,
            provenance: ProcedureProvenance {
                origin: "test".into(),
                proposed_by: "test".into(),
                approved_by: None,
            },
            supersedes: None,
        });

        assert!(
            compose(
                &registry,
                &GoalSpec {
                    intent: "get_order_status".into(),
                    produces: vec!["general_deep".into()],
                    available_slots: vec![],
                    allowed_capabilities: vec![],
                    allow_effects: false,
                },
                CompositionBudget::default(),
            )
            .is_none(),
            "cross-intent composition can produce plausible but semantically wrong replies"
        );
    }

    #[test]
    fn tier1_is_scored_vector_knn_with_margin_gate() {
        let embedder = default_situation_embedder();
        let mut reg = Registry::default();
        reg.register_ability(AbilityContract::pure("Express.Template"));

        let greeting = situation_key("unauth", "greeting", vec![], vec![], None, 0, None);
        let login = situation_key("unauth", "login", vec![], vec![], None, 0, None);

        let mut greeting_proc = ProcedureSpec {
            id: "greet".into(),
            version: "1".into(),
            tenant_id: "t".into(),
            situation_hash: situation_hash(&greeting),
            situation_filter: SituationFilter {
                state: Some("unauth".into()),
                intent_class: Some("greeting".into()),
                ..Default::default()
            },
            situation_embedding: embed_situation(&embedder, &greeting).unwrap(),
            path: AbilityPath::seq(["Express.Template"]),
            contract: AbilityContract::pure("greet"),
            tool_deps: vec![],
            prompt_deps: vec![],
            evidence: ProcedureEvidence::default(),
            status: ProcedureStatus::Promoted,
            provenance: ProcedureProvenance {
                origin: "test".into(),
                proposed_by: "test".into(),
                approved_by: None,
            },
            supersedes: None,
        };
        // Returning visit: different full hash (Early bucket), same near vector.
        let returning = situation_key("unauth", "greeting", vec![], vec![], None, 1, Some(60));
        assert_ne!(situation_hash(&greeting), situation_hash(&returning));

        reg.register_procedure(greeting_proc.clone());
        let hit = lookup_tier_with_embedder(&reg, &returning, &embedder);
        assert_eq!(hit.tier, LookupTier::Tier1);
        assert_eq!(hit.procedure_id.as_deref(), Some("greet"));
        assert!(hit.margin >= TIER1_MARGIN_THRESHOLD);

        // Competing near-duplicate collapses margin → refuse Tier1.
        greeting_proc.id = "greet-b".into();
        greeting_proc.situation_hash = format!("{}b", greeting_proc.situation_hash);
        reg.register_procedure(greeting_proc);
        let contested = lookup_tier_with_embedder(&reg, &returning, &embedder);
        assert_eq!(contested.tier, LookupTier::Tier3);
        assert!(contested.margin < TIER1_MARGIN_THRESHOLD);

        // Unrelated intent must not win just because something exists.
        let login_proc = ProcedureSpec {
            id: "login-path".into(),
            version: "1".into(),
            tenant_id: "t".into(),
            situation_hash: situation_hash(&login),
            situation_filter: SituationFilter {
                state: Some("unauth".into()),
                intent_class: Some("login".into()),
                ..Default::default()
            },
            situation_embedding: embed_situation(&embedder, &login).unwrap(),
            path: AbilityPath::seq(["Express.Template"]),
            contract: AbilityContract::pure("login-path"),
            tool_deps: vec![],
            prompt_deps: vec![],
            evidence: ProcedureEvidence::default(),
            status: ProcedureStatus::Promoted,
            provenance: ProcedureProvenance {
                origin: "test".into(),
                proposed_by: "test".into(),
                approved_by: None,
            },
            supersedes: None,
        };
        let mut only_login = Registry::default();
        only_login.register_ability(AbilityContract::pure("Express.Template"));
        only_login.register_procedure(login_proc.clone());
        let miss = lookup_tier_with_embedder(&only_login, &returning, &embedder);
        // Single candidate: margin = score - 0. Accept only if score itself clears 0.8.
        // HashEmbedder near-texts for greeting vs login should not clear that bar.
        assert!(
            miss.tier == LookupTier::Tier3 || miss.margin < TIER1_MARGIN_THRESHOLD,
            "unrelated intent must not clear the Tier1 margin gate"
        );

        // Even a perfect vector match cannot cross hard state/slot/capability boundaries.
        let mut forbidden = login_proc;
        forbidden.id = "forbidden-perfect-vector".into();
        forbidden.situation_embedding = embed_situation(&embedder, &returning).unwrap();
        forbidden.situation_filter.state = Some("authenticated".into());
        forbidden.situation_filter.required_slots = vec!["admin_token".into()];
        forbidden.situation_filter.capability_tags = vec!["admin.read".into()];
        let mut hard_filtered = Registry::default();
        hard_filtered.register_ability(AbilityContract::pure("Express.Template"));
        hard_filtered.register_procedure(forbidden);
        assert_eq!(
            lookup_tier_with_embedder(&hard_filtered, &returning, &embedder).tier,
            LookupTier::Tier3
        );
    }

    #[test]
    fn promote_requires_evidence() {
        let p = Proposal {
            id: "p1".into(),
            situation_hash: "h".into(),
            sigma: None,
            path: AbilityPath::seq(["Express.Template"]),
            observations: 1,
            successes: 1,
            effectful: false,
            status: ProcedureStatus::Proposed,
        };
        let mut reg = Registry::default();
        reg.register_ability(AbilityContract::pure("Express.Template"));
        assert!(promote(&reg, &p, 3, 0.8, false, &default_situation_embedder()).is_err());
        let p2 = Proposal {
            observations: 5,
            successes: 5,
            ..p
        };
        assert!(promote(&reg, &p2, 3, 0.8, false, &default_situation_embedder()).is_ok());
    }

    #[test]
    fn promotion_captures_tool_dependencies() {
        let mut reg = Registry::default();
        reg.register_tool(crate::tenant::ToolSpec {
            id: "write_order".into(),
            name: "write_order".into(),
            version: "1".into(),
            capability_tags: vec!["orders.write".into()],
            effectful: true,
            idempotent: false,
            dry_run_available: false,
            params: vec![],
            output_semantics: crate::tenant::OutputSpec {
                fields: IndexMap::new(),
                role_hint: None,
            },
            continuations: vec![],
            errors: vec![],
        });
        let mut ability =
            AbilityContract::effect("orders.write").with_tool_deps(vec!["write_order".into()]);
        ability.prompt_hash = Some("orders-write-prompt-v1".into());
        reg.register_ability(ability);
        let proposal = Proposal {
            id: "dependent".into(),
            situation_hash: "hash".into(),
            sigma: None,
            path: AbilityPath::seq(["orders.write"]),
            observations: 5,
            successes: 5,
            effectful: true,
            status: ProcedureStatus::Candidate,
        };
        let promoted =
            promote(&reg, &proposal, 3, 0.8, true, &default_situation_embedder()).unwrap();
        assert_eq!(promoted.tool_deps, vec!["write_order"]);
        assert_eq!(promoted.contract.tool_deps, promoted.tool_deps);
        assert_eq!(promoted.prompt_deps, vec!["orders-write-prompt-v1"]);
    }

    #[test]
    fn repeated_situation_warms_to_tier0_and_near_hits_tier1() {
        // A non-effectful deterministic path (pure express) for a novel situation.
        let mut reg = Registry::default();
        reg.register_ability(AbilityContract::pure("Express.Synthesize"));
        let mut proposals = ProposalMap::new();
        let sigma = situation_key(
            "authenticated",
            "invoices",
            vec![],
            vec!["orders.list".into()],
            None,
            0,
            None,
        );
        let path = AbilityPath::seq(["Express.Synthesize"]);

        // First observations accumulate but do not promote (asymmetric gate: slow to promote).
        for _ in 0..(PROMOTE_MIN_OBSERVATIONS - 1) {
            let outcome = observe_and_promote(
                &mut proposals,
                &mut reg,
                &sigma,
                path.clone(),
                false,
                true,
                false,
                &default_situation_embedder(),
            );
            assert!(matches!(outcome, ObserveOutcome::Accumulating { .. }));
        }
        // Exact σ still cold until the gate clears.
        assert_eq!(lookup_tier(&reg, &sigma).tier, LookupTier::Tier3);

        // The observation that clears the gate promotes the path into the live registry.
        let outcome = observe_and_promote(
            &mut proposals,
            &mut reg,
            &sigma,
            path.clone(),
            false,
            true,
            false,
            &default_situation_embedder(),
        );
        assert!(matches!(outcome, ObserveOutcome::Promoted { .. }));

        // The exact same σ now hits Tier-0.
        assert_eq!(lookup_tier(&reg, &sigma).tier, LookupTier::Tier0);

        // The same situation on a later turn (different turn bucket) near-hits Tier-1, because the
        // promoted procedure embedded the real σ (bucket-independent near-text).
        let later = situation_key(
            "authenticated",
            "invoices",
            vec![],
            vec!["orders.list".into()],
            None,
            9,
            Some(120),
        );
        assert_ne!(situation_hash(&sigma), situation_hash(&later));
        assert_eq!(lookup_tier(&reg, &later).tier, LookupTier::Tier1);
    }

    #[test]
    fn effectful_path_does_not_auto_promote_without_approval() {
        let mut reg = Registry::default();
        reg.register_tool(crate::tenant::ToolSpec {
            id: "send_otp".into(),
            name: "send_otp".into(),
            version: "1".into(),
            capability_tags: vec!["auth.otp.send".into()],
            effectful: true,
            idempotent: false,
            dry_run_available: true,
            params: vec![],
            output_semantics: crate::tenant::OutputSpec {
                fields: IndexMap::new(),
                role_hint: None,
            },
            continuations: vec![],
            errors: vec![],
        });
        let mut proposals = ProposalMap::new();
        let sigma = situation_key("unauthenticated", "login", vec![], vec![], None, 0, None);
        let path = AbilityPath::seq(["send_otp"]);
        assert!(path_is_effectful(&reg, &path));

        for _ in 0..(PROMOTE_MIN_OBSERVATIONS + 2) {
            let outcome = observe_and_promote(
                &mut proposals,
                &mut reg,
                &sigma,
                path.clone(),
                true,
                true,
                false,
                &default_situation_embedder(),
            );
            assert!(
                matches!(outcome, ObserveOutcome::Accumulating { .. }),
                "effectful path must not promote without tenant approval"
            );
        }
        assert_eq!(lookup_tier(&reg, &sigma).tier, LookupTier::Tier3);
    }

    #[test]
    fn read_only_tool_path_can_learn_without_effect_approval() {
        let world = crate::World::demo_tenant("tenant-read-learning");
        let path = AbilityPath::seq(["crm.clients.query"]);
        assert!(!path_is_effectful(&world.registry, &path));
    }
}
