use aelio_agent::abilities::registry::Registry;
use aelio_agent::abilities::{evaluate_warm_match, Gate, MatchingConfig, Outcome};
use aelio_agent::contract::{AbilityPath, PathStep};
use aelio_agent::reuse_metrics;
use aelio_agent::tenant::{
    Completeness, EffectClass, OutputSpec, ParamSource, ParamSpec, ToolContract, ToolSpec,
};
use aelio_agent::types::{LookupTier, Sensitivity};
use indexmap::IndexMap;
use std::sync::{Mutex, OnceLock};

fn metric_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn registry() -> Registry {
    let mut registry = Registry::default();
    registry
        .register_tool(ToolSpec {
            id: "people.mean.gt".into(),
            name: "people.mean.gt".into(),
            version: "1".into(),
            capability_tags: vec![],
            contract: Some(ToolContract {
                effect_class: EffectClass::Read,
                completeness: Completeness::Complete,
                returns_entity: "person".into(),
                pushdown: vec![],
                max_result_rows: Some(100),
                row_scoped: false,
            }),
            effect: None,
            effectful: false,
            idempotent: true,
            dry_run_available: false,
            params: vec![slot("field"), slot("threshold")],
            output_semantics: OutputSpec {
                fields: IndexMap::new(),
                role_hint: None,
            },
            continuations: vec![],
            errors: vec![],
        })
        .unwrap();
    registry
}

fn slot(name: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        type_name: "str".into(),
        required: true,
        constraint: None,
        source: ParamSource::Slot { name: name.into() },
        repair: None,
        prompt_hint: None,
        sensitivity: Sensitivity::None,
        default: None,
        depends_on: vec![],
    }
}

fn dynamic_path() -> AbilityPath {
    AbilityPath {
        steps: vec![PathStep {
            ability_id: "people.mean.gt".into(),
            args: IndexMap::new(),
        }],
    }
}

fn static_height_path() -> AbilityPath {
    AbilityPath {
        steps: vec![PathStep {
            ability_id: "people.mean.gt".into(),
            args: indexmap::indexmap! {
                "field".into() => serde_json::json!("height"),
                "operator".into() => serde_json::json!("gt"),
                "aggregate".into() => serde_json::json!("mean"),
            },
        }],
    }
}

fn slots(field: &str, threshold: Option<u64>) -> IndexMap<String, serde_json::Value> {
    let mut slots = indexmap::indexmap! {
        "field".into() => serde_json::json!(field),
    };
    if let Some(threshold) = threshold {
        slots.insert("threshold".into(), serde_json::json!(threshold));
    }
    slots
}

fn decide(
    tier: LookupTier,
    path: &AbilityPath,
    utterance: &str,
    slots: IndexMap<String, serde_json::Value>,
) -> aelio_agent::abilities::MatchDecision {
    evaluate_warm_match(
        MatchingConfig::default(),
        tier,
        &registry(),
        path,
        utterance,
        &slots,
    )
}

#[test]
fn c1_extra_bangalore_constraint_rejects_at_residue() {
    let decision = decide(
        LookupTier::Tier0,
        &dynamic_path(),
        "average height of people over 28 in Bangalore",
        slots("height", Some(28)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::Residue);
}

#[test]
fn c2_missing_required_filter_rejects_at_bind() {
    let decision = decide(
        LookupTier::Tier0,
        &dynamic_path(),
        "average height of people",
        slots("height", None),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::Bind);
}

#[test]
fn c3_median_cannot_execute_cached_mean() {
    let decision = decide(
        LookupTier::Tier0,
        &static_height_path(),
        "median height of people over 28",
        slots("height", Some(28)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::Verification);
}

#[test]
fn c4_inclusive_boundary_cannot_bind_to_strict_gt() {
    let decision = decide(
        LookupTier::Tier0,
        &static_height_path(),
        "average height of people >= 28",
        slots("height", Some(28)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::BindingCorrectness);
}

#[test]
fn c5_static_height_cannot_serve_weight_request() {
    let decision = decide(
        LookupTier::Tier0,
        &static_height_path(),
        "average weight of people over 40",
        slots("weight", Some(40)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::Bind);
}

#[test]
fn c6_people_candidate_cannot_serve_employees_request() {
    let decision = decide(
        LookupTier::Tier0,
        &dynamic_path(),
        "average height of employees over 28",
        slots("height", Some(28)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::Bind);
}

#[test]
fn c7_null_policy_requires_distinct_structure() {
    let decision = decide(
        LookupTier::Tier0,
        &dynamic_path(),
        "average height of people over 28 including unknown heights",
        slots("height", Some(28)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::Structure);
}

#[test]
fn c8_implicit_aggregation_requires_plan_presentation() {
    let decision = decide(
        LookupTier::Tier0,
        &dynamic_path(),
        "typical height of people over 28",
        slots("height", Some(28)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::ImplicitTerm);
}

#[test]
fn c9_high_scoring_neighbour_fails_closed_on_residue() {
    let decision = decide(
        LookupTier::Tier1,
        &dynamic_path(),
        "average height of people over 28 in Bangalore",
        slots("height", Some(28)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::FailClosed);
}

#[test]
fn matching_disabled_fails_safe_to_cold_authoring() {
    let decision = evaluate_warm_match(
        MatchingConfig {
            matching_enabled: false,
        },
        LookupTier::Tier0,
        &registry(),
        &dynamic_path(),
        "average height of people over 28",
        &slots("height", Some(28)),
    );
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::MatchingDisabled);
}

#[test]
fn b4_dynamic_skeleton_reuses_with_new_weight_binding() {
    let _guard = metric_test_lock().lock().unwrap();
    let before = reuse_metrics::snapshot();
    let decision = decide(
        LookupTier::Tier0,
        &dynamic_path(),
        "average weight of people over 40",
        slots("weight", Some(40)),
    );
    reuse_metrics::record_match_decision(&decision);
    reuse_metrics::record_lookup(LookupTier::Tier0, "b4");
    let after = reuse_metrics::snapshot();
    assert_eq!(decision.outcome, Outcome::Accepted);
    assert_eq!(decision.gate, Gate::Accepted);
    assert_eq!(after.cold_executions, before.cold_executions);
    assert_eq!(after.warm_hits, before.warm_hits + 1);
    assert_eq!(
        after.gate_decisions_total[&(Gate::Accepted, Outcome::Accepted)],
        before
            .gate_decisions_total
            .get(&(Gate::Accepted, Outcome::Accepted))
            .copied()
            .unwrap_or(0)
            + 1
    );
}

#[test]
fn b5_between_range_forces_cold_authoring() {
    let _guard = metric_test_lock().lock().unwrap();
    let before = reuse_metrics::snapshot();
    let decision = decide(
        LookupTier::Tier0,
        &dynamic_path(),
        "average height of people between 28 and 40",
        slots("height", Some(28)),
    );
    reuse_metrics::record_match_decision(&decision);
    reuse_metrics::record_lookup(LookupTier::Tier3, "b5");
    let after = reuse_metrics::snapshot();
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.gate, Gate::Structure);
    assert_eq!(after.cold_executions, before.cold_executions + 1);
    assert_eq!(
        after.gate_decisions_total[&(Gate::Structure, Outcome::Rejected)],
        before
            .gate_decisions_total
            .get(&(Gate::Structure, Outcome::Rejected))
            .copied()
            .unwrap_or(0)
            + 1
    );
}
