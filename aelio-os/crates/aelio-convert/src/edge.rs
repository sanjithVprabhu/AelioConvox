//! Conversion graph data model (§13). An edge is `(tenant, flow_id, producer_nid, consumer_nid)` —
//! **nid-based, version-free** (§4.1.6 as amended), so edges survive flow edits. This ties the rule
//! engine (§14) and the gate (§16) into a lifecycle-bearing record, and adds the `rejected` state's
//! anti-proposal-loop (§13.1, §15 backoff).

use crate::gate::{self, Digest, Evidence, Thresholds, Tier};
use crate::lifecycle::{transition, Status, Trigger};
use crate::rules::Rule;
use aelio_sol::SolValue;
use std::collections::BTreeMap;

/// §4.1.6 edge identity. Node ids are minted once at authoring time and survive edits.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdgeId {
    pub tenant: String,
    pub flow_id: String,
    pub producer_nid: String,
    pub consumer_nid: String,
}

impl EdgeId {
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.tenant, self.flow_id, self.producer_nid, self.consumer_nid
        )
    }
}

/// Per-edge `on_parse_fail` policy (§14): `error` propagates `Convert.RuleFail` at the consumer
/// boundary; `default(v)` substitutes a value.
#[derive(Debug, Clone)]
pub enum OnParseFail {
    Error,
    Default(SolValue),
}

/// Sensitivity (§15, shared with §28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensitivity {
    Public,
    Internal,
    Pii,
    Secret,
}

/// A conversion edge (§13 metadata). `rules_hash` keys the `rejected` short-circuit.
#[derive(Debug, Clone)]
pub struct ConversionEdge {
    pub id: EdgeId,
    pub conversion_id: String,
    pub version: u32,
    pub status: Status,
    pub rules: Vec<Rule>,
    pub rules_hash: String,
    /// Structural imprint (`~hash`) of the producer output shape (§4.1.5 — lookup/bucket key only).
    pub from_signature: String,
    pub to_digest: Digest,
    pub on_parse_fail: OnParseFail,
    pub sensitivity: Sensitivity,
    pub evidence: Evidence,
}

impl ConversionEdge {
    /// Apply the gate lifecycle transition to this edge (§13.1 / App K).
    pub fn advance(&mut self, trigger: Trigger) -> Result<(), &'static str> {
        self.status = transition(self.status, trigger)?;
        Ok(())
    }

    /// Evidence-authoritative lifecycle transition. Production code should use this method rather
    /// than manufacturing threshold triggers directly.
    pub fn advance_checked(
        &mut self,
        trigger: Trigger,
        tier: Tier,
        deployer_approved: bool,
        thresholds: &Thresholds,
    ) -> Result<(), String> {
        gate::validate_thresholds(thresholds)?;
        match trigger {
            Trigger::ShadowThresholdsMet { .. } => {
                if !gate::shadow_to_canary(&self.evidence, thresholds) {
                    return Err("shadow evidence does not meet promotion threshold".into());
                }
                let approved = tier == Tier::Auto || deployer_approved;
                self.advance(Trigger::ShadowThresholdsMet { approved })
                    .map_err(str::to_owned)
            }
            Trigger::CanaryThresholdsMet => {
                if !gate::canary_to_promoted(&self.evidence, thresholds) {
                    return Err("canary evidence does not meet promotion threshold".into());
                }
                self.advance(trigger).map_err(str::to_owned)
            }
            other => self.advance(other).map_err(str::to_owned),
        }
    }

    /// Warm application at a use (§14 / §13.1): run the rules; on `RuleFail`, honor `on_parse_fail`.
    pub fn apply_at_use(&self, input: &SolValue) -> Result<SolValue, ConvertUseError> {
        match crate::rules::apply_rules(&self.rules, input) {
            Ok(out) => Ok(out),
            Err(fail) => match &self.on_parse_fail {
                OnParseFail::Error => Err(ConvertUseError::RuleFail {
                    rule_index: fail.rule_index,
                    detail: fail.detail,
                }),
                OnParseFail::Default(v) => Ok(v.clone()),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConvertUseError {
    RuleFail { rule_index: usize, detail: String },
}

/// The canonical hash of a rule list — the `rejected`/proposal-dedup key (§13.1).
pub fn rules_hash(rules: &[Rule]) -> String {
    // Serialize rules to a stable Sol shape, then hash. (A structural, deterministic fingerprint.)
    let list = SolValue::List(rules.iter().map(rule_to_sol).collect());
    aelio_sol::value_hash(&list)
}

fn rule_to_sol(r: &Rule) -> SolValue {
    // A stable tag+payload encoding; only identity matters (dedup), not round-trip.
    match r {
        Rule::Rename { from, to } => {
            tagged("rename", &[("from", path_str(from)), ("to", path_str(to))])
        }
        Rule::Drop { path } => tagged("drop", &[("path", path_str(path))]),
        Rule::Keep { paths } => SolValue::map([
            ("op", SolValue::str("keep")),
            (
                "paths",
                SolValue::List(paths.iter().map(|p| SolValue::str(path_str(p))).collect()),
            ),
        ]),
        Rule::Default { path, v } => SolValue::map([
            ("op", SolValue::str("default")),
            ("path", SolValue::str(path_str(path))),
            ("v", v.clone()),
        ]),
        Rule::Cast { path, to, mode } => tagged(
            "cast",
            &[
                ("path", path_str(path)),
                ("to", to.clone()),
                ("mode", mode.clone().unwrap_or_default()),
            ],
        ),
        Rule::Wrap { path, key } => {
            tagged("wrap", &[("path", path_str(path)), ("key", key.clone())])
        }
        Rule::Unwrap { path } => tagged("unwrap", &[("path", path_str(path))]),
        Rule::MapEnum { path, table } => SolValue::map([
            ("op", SolValue::str("map_enum")),
            ("path", SolValue::str(path_str(path))),
            ("table", SolValue::Map(table.clone())),
        ]),
        Rule::PathCopy { from, to } => tagged(
            "path_copy",
            &[("from", path_str(from)), ("to", path_str(to))],
        ),
        Rule::ConstSet { path, v } => SolValue::map([
            ("op", SolValue::str("const_set")),
            ("path", SolValue::str(path_str(path))),
            ("v", v.clone()),
        ]),
        Rule::Trim { path } => tagged("trim", &[("path", path_str(path))]),
    }
}

fn tagged(op: &str, fields: &[(&str, String)]) -> SolValue {
    let mut m: BTreeMap<String, SolValue> = BTreeMap::new();
    m.insert("op".into(), SolValue::str(op));
    for (k, v) in fields {
        m.insert((*k).into(), SolValue::str(v.clone()));
    }
    SolValue::Map(m)
}

fn path_str(p: &aelio_sol::Path) -> String {
    p.to_string()
}

/// Edge inspector (§26): "why did this Convert fire" + full evidence, straight from the edge record.
pub fn inspect(edge: &ConversionEdge) -> String {
    let fab = if crate::rules::any_fabricating(&edge.rules) {
        " (fabricating)"
    } else {
        ""
    };
    let digest_keys: Vec<String> = edge
        .to_digest
        .required
        .iter()
        .map(|(k, t)| format!("{k}:{t}"))
        .collect();
    format!(
        "edge {}\n  conversion {}@{}  status={:?}  sensitivity={:?}\n  rules={}{}  on_parse_fail={:?}\n  from_signature={}\n  to_digest={{{}}}\n  evidence: shadow {}/{}@{:.0}%  canary {}@{:.0}%  guard_violations={}",
        edge.id.key(),
        edge.conversion_id,
        edge.version,
        edge.status,
        edge.sensitivity,
        edge.rules.len(),
        fab,
        edge.on_parse_fail,
        short_hash(&edge.from_signature),
        digest_keys.join(", "),
        edge.evidence.shadow_distinct,
        edge.evidence.shadow_distinct,
        edge.evidence.shadow_validation_rate * 100.0,
        edge.evidence.canary_distinct,
        edge.evidence.canary_success_rate * 100.0,
        edge.evidence.attributed_guard_violations,
    )
}

fn short_hash(s: &str) -> String {
    if s.chars().count() > 14 {
        format!("{}…", s.chars().take(14).collect::<String>())
    } else {
        s.to_string()
    }
}

/// Anti-proposal-loop registry (§13.1, §15): a `rejected` rules_hash short-circuits repeat proposals
/// to backoff; a *different* rules_hash starts fresh.
#[derive(Default)]
pub struct RejectedRegistry {
    rejected: BTreeMap<String, String>, // rules_hash → reason
}

impl RejectedRegistry {
    pub fn reject(&mut self, rules_hash: impl Into<String>, reason: impl Into<String>) {
        self.rejected.insert(rules_hash.into(), reason.into());
    }

    /// True if a proposal with this rules_hash was already rejected (⇒ short-circuit to backoff).
    pub fn is_rejected(&self, rules_hash: &str) -> bool {
        self.rejected.contains_key(rules_hash)
    }
}
