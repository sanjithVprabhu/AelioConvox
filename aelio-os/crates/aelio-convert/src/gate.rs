//! The generic promotion gate (§16): reach-based tiers, phase predicates (structural / shadow /
//! canary / promotion), and the v0 thresholds (§16.4). Each phase proves *one* thing honestly; the
//! irreducibly-semantic residue is routed to humans by reach (§16.2).

use crate::rules::{any_fabricating, apply_rules, Rule};
use aelio_sol::SolValue;
use std::collections::BTreeMap;

/// Sensitivity tiers (§16.2). Reviewed = human sees the transform before it governs anything with
/// reach; locked = deployer-authored only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Auto,
    Reviewed,
    Locked,
}

/// The static facts a taint walk supplies about an edge's downstream reach (§16.2). Computable
/// because plans are static, paths literal, effect classes declared, and the call graph acyclic.
#[derive(Debug, Clone, Copy, Default)]
pub struct Reach {
    /// Any `write`/external Call reachable downstream of the consumer nid.
    pub reaches_write_or_external: bool,
    pub is_pii: bool,
    pub is_secret: bool,
}

/// Classify the tier from reach + the rules. Fabricating rules (`default`/`const_set`) escalate to
/// reviewed regardless of reach (§14). Expression-to-current-user is not "reach" (the §16.2
/// carve-out) — the caller is responsible for not setting `reaches_*` for pure conversational edges.
pub fn classify_tier(reach: &Reach, rules: &[Rule]) -> Tier {
    if reach.is_secret {
        Tier::Locked
    } else if reach.is_pii || reach.reaches_write_or_external || any_fabricating(rules) {
        Tier::Reviewed
    } else {
        Tier::Auto
    }
}

/// The consumer's input digest — required keys + their fundamental-type signatures (§16.1.2). This
/// is the shadow-phase oracle: no old path needed.
#[derive(Debug, Clone)]
pub struct Digest {
    pub required: BTreeMap<String, String>,
}

impl Digest {
    pub fn new<I, K>(pairs: I) -> Digest
    where
        I: IntoIterator<Item = (K, &'static str)>,
        K: Into<String>,
    {
        Digest { required: pairs.into_iter().map(|(k, t)| (k.into(), t.to_string())).collect() }
    }

    /// Does a produced Sol satisfy the consumer digest (all required keys present with the right
    /// fundamental type)?
    pub fn satisfied_by(&self, sol: &SolValue) -> bool {
        let Some(map) = sol.as_map() else { return false };
        self.required.iter().all(|(k, ty)| {
            map.get(k).map(|v| v.type_tag().signature() == ty).unwrap_or(false)
        })
    }
}

/// Phase 1 — Structural (§16.1.1): rules are well-formed and closed. (Parsing already proved
/// closure + non-computation; this is the gate's explicit hook, and where `type_map` consistency
/// would live.) Returns Ok on pass.
pub fn structural_ok(rules: &[Rule]) -> Result<(), String> {
    if rules.is_empty() {
        return Err("empty rule list".into());
    }
    Ok(())
}

/// Phase 2 — Shadow = validate-without-consume (§16.1.2). Run the converter over one live input,
/// check the output against the consumer digest, discard it. A `RuleFail` counts as a validation
/// failure (robustness over the real distribution).
pub fn shadow_validate(rules: &[Rule], input: &SolValue, digest: &Digest) -> bool {
    match apply_rules(rules, input) {
        Ok(out) => digest.satisfied_by(&out),
        Err(_) => false,
    }
}

/// v0 gate thresholds (§16.4), deployer-tunable within bounds.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub shadow_min_distinct: u64,
    pub shadow_min_rate: f64,
    pub canary_min_distinct: u64,
    pub canary_min_success: f64,
    pub demote_failure_rate: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            shadow_min_distinct: 20,
            shadow_min_rate: 0.95,
            canary_min_distinct: 20,
            canary_min_success: 0.98,
            demote_failure_rate: 0.02,
        }
    }
}

/// Evidence (§13.2) — all counts over **distinct input hashes** (never raw runs, so evidence can't
/// be inflated by repetition).
#[derive(Debug, Clone, Copy, Default)]
pub struct Evidence {
    pub shadow_distinct: u64,
    pub shadow_validation_rate: f64,
    pub canary_distinct: u64,
    pub canary_success_rate: f64,
    pub attributed_guard_violations: u64,
}

pub fn shadow_to_canary(ev: &Evidence, th: &Thresholds) -> bool {
    ev.shadow_distinct >= th.shadow_min_distinct && ev.shadow_validation_rate >= th.shadow_min_rate
}

pub fn canary_to_promoted(ev: &Evidence, th: &Thresholds) -> bool {
    ev.canary_distinct >= th.canary_min_distinct
        && ev.canary_success_rate >= th.canary_min_success
        && ev.attributed_guard_violations == 0
}

pub fn should_demote(canary_failure_rate: f64, guard_violation: bool, th: &Thresholds) -> bool {
    guard_violation || canary_failure_rate > th.demote_failure_rate
}
