//! Judge.* — keeps the model honest. Confidence is first-class.

use crate::contract::Predicate;
use crate::policy::{eval_predicate, PolicyCtx};
use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Confidence {
    pub margin: f64,
    pub entropy: f64,
    pub calibrated_p: f64,
}

/// Margin between top two scores + entropy of distribution.
pub fn confidence(scores: &[f64]) -> Confidence {
    if scores.is_empty() {
        return Confidence {
            margin: 0.0,
            entropy: 0.0,
            calibrated_p: 0.0,
        };
    }
    let mut sorted = scores.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let top = sorted[0];
    let second = sorted.get(1).copied().unwrap_or(0.0);
    let margin = top - second;

    // Shannon entropy of softmax-ish normalized scores
    let sum: f64 = scores.iter().map(|s| s.max(0.0)).sum::<f64>().max(1e-9);
    let mut entropy = 0.0;
    for s in scores {
        let p = (s.max(0.0) / sum).max(1e-12);
        entropy -= p * p.ln();
    }
    // crude calibration: high margin + low entropy → high p
    let calibrated_p = (margin * (1.0 / (1.0 + entropy))).clamp(0.0, 1.0);
    Confidence {
        margin,
        entropy,
        calibrated_p,
    }
}

pub fn should_escalate(conf: &Confidence, threshold: f64) -> bool {
    conf.margin < threshold
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Sufficiency {
    Complete,
    Missing { slots: Vec<String> },
}

pub fn sufficient(present: &[String], required: &[String]) -> Sufficiency {
    let missing: Vec<String> = required
        .iter()
        .filter(|r| !present.iter().any(|p| p == *r))
        .cloned()
        .collect();
    if missing.is_empty() {
        Sufficiency::Complete
    } else {
        Sufficiency::Missing { slots: missing }
    }
}

pub fn postcondition(state: &PolicyCtx, invariant: &Predicate) -> AelioResult<()> {
    if eval_predicate(invariant, state) {
        Ok(())
    } else {
        Err(AelioError::new(
            ReasonCode::Validation,
            "postcondition violated",
        ))
    }
}

/// Outcome from real behavioural signals — never LLM self-grade.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutcomeSignals {
    pub tool_non_error: bool,
    pub repair_detected: bool,
    pub flow_terminal: bool,
    pub abandoned: bool,
    pub latency_ms: u64,
    pub token_cost: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeScore {
    pub score: f64,
    pub labels: Vec<String>,
}

pub fn outcome(signals: &OutcomeSignals) -> OutcomeScore {
    let mut score: f64 = 0.5;
    let mut labels = Vec::new();
    if signals.tool_non_error {
        score += 0.15;
        labels.push("tool_ok".into());
    }
    if signals.flow_terminal {
        score += 0.25;
        labels.push("flow_terminal".into());
    }
    if signals.repair_detected {
        score -= 0.4;
        labels.push("repair".into());
    }
    if signals.abandoned {
        score -= 0.35;
        labels.push("abandon".into());
    }
    // efficiency is tracked but not correctness
    if signals.latency_ms > 1500 {
        labels.push("slow".into());
    }
    OutcomeScore {
        score: score.clamp(0.0, 1.0),
        labels,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceClaim {
    pub id: String,
    pub value: Value,
    /// Stable source descriptor such as `tool:<id>:<signature>` or `state:<id>`.
    pub provenance: String,
}

/// Deterministic groundedness gate: every emitted reference must name a supplied claim, and a
/// factual synthesis must cite at least one claim. Semantic entailment can be layered on after this
/// structural check, but cannot substitute for provenance.
pub fn verify_claim_refs(claims: &[EvidenceClaim], references: &[String]) -> AelioResult<()> {
    let known: HashSet<&str> = claims.iter().map(|claim| claim.id.as_str()).collect();
    if !claims.is_empty() && references.is_empty() {
        return Err(AelioError::new(
            ReasonCode::Unresolved,
            "synthesized response did not cite any evidence claim",
        ));
    }
    if let Some(unknown) = references
        .iter()
        .find(|reference| !known.contains(reference.as_str()))
    {
        return Err(AelioError::new(
            ReasonCode::Unresolved,
            format!("synthesized response cited unknown claim `{unknown}`"),
        ));
    }
    Ok(())
}

/// Legacy diagnostic only. Production synthesis uses `verify_claim_refs` above.
pub fn groundedness(utterance: &str, evidence_texts: &[&str]) -> (f64, Vec<String>) {
    let claim_words: Vec<&str> = utterance
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .collect();
    if claim_words.is_empty() {
        return (1.0, vec![]);
    }
    let corpus = evidence_texts.join(" ").to_lowercase();
    let mut unsupported = Vec::new();
    let mut supported = 0usize;
    for w in &claim_words {
        if corpus.contains(&w.to_lowercase()) {
            supported += 1;
        } else {
            unsupported.push((*w).to_string());
        }
    }
    let p = supported as f64 / claim_words.len() as f64;
    (p, unsupported)
}

pub fn confidence_to_value(c: &Confidence) -> Value {
    Value::Map(indexmap::indexmap! {
        "margin".into() => Value::Float(c.margin),
        "entropy".into() => Value::Float(c.entropy),
        "calibrated_p".into() => Value::Float(c.calibrated_p),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_margin_escalates() {
        let c = confidence(&[0.79, 0.74]);
        assert!(c.margin < 0.1);
        assert!(should_escalate(&c, 0.15));
    }

    #[test]
    fn repair_hurts_outcome() {
        let good = outcome(&OutcomeSignals {
            tool_non_error: true,
            flow_terminal: true,
            ..Default::default()
        });
        let bad = outcome(&OutcomeSignals {
            tool_non_error: true,
            repair_detected: true,
            ..Default::default()
        });
        assert!(good.score > bad.score);
    }

    #[test]
    fn claim_refs_fail_closed() {
        let claims = vec![EvidenceClaim {
            id: "claim-1".into(),
            value: Value::str("paid"),
            provenance: "tool:invoice.get:sig".into(),
        }];
        assert!(verify_claim_refs(&claims, &["claim-1".into()]).is_ok());
        assert!(verify_claim_refs(&claims, &[]).is_err());
        assert!(verify_claim_refs(&claims, &["invented".into()]).is_err());
    }
}
