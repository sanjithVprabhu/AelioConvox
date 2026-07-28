//! Pathway registry & selection (§18). Selection **is a classifier and gets classifier hygiene**:
//! a pick requires top-1 ≥ τ AND margin(top1−top2) ≥ δ AND entropy below a ceiling; failing any one
//! routes to a **mandatory declared fallback** (every decision point declares one, or plan-time
//! reject). Fallback rate is the de-facto drift detector (§21). Cap: 8 pathways per decision point.

/// One scored candidate at a decision point. Scores are comparable only within one pinned embedding
/// space (§18 — the Lighthouse score-space bug is made impossible by re-embed-on-model-upgrade).
#[derive(Debug, Clone)]
pub struct PathwayScore {
    pub pathway_id: String,
    pub score: f64,
}

/// Hygiene thresholds — calibrated empirically per tenant, never copied constants (§18).
#[derive(Debug, Clone, Copy)]
pub struct Hygiene {
    pub tau: f64,
    pub delta: f64,
    pub entropy_ceiling: f64,
}

/// Max pathways per decision point (§18, v0).
pub const MAX_PATHWAYS: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    /// A confident pick, with the metrics that justified it (ledgered per §12.2).
    Picked {
        pathway_id: String,
        top: f64,
        margin: f64,
        entropy: f64,
    },
    /// All hygiene checks are ANDed; any failure routes to the declared fallback.
    Fallback {
        reason: FallbackReason,
        entropy: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// No candidates at all.
    Empty,
    /// Top score below τ.
    LowConfidence,
    /// margin(top1−top2) below δ.
    ThinMargin,
    /// Distribution too flat (entropy above ceiling).
    HighEntropy,
    /// NaN/∞, duplicate ids, or an invalid threshold configuration.
    InvalidInput,
    /// The runtime received more candidates than the declared hard cap.
    TooManyCandidates,
}

/// A decision point declaration. `fallback` is mandatory — a decision point without one is
/// plan-time rejected (see [`validate_decision_point`]).
#[derive(Debug, Clone)]
pub struct DecisionPoint {
    pub id: String,
    pub fallback_pathway: String,
    pub hygiene: Hygiene,
}

/// Plan-time validation (§18): every decision point declares a fallback, and no more than 8
/// candidates may be presented.
pub fn validate_decision_point(dp: &DecisionPoint, candidate_count: usize) -> Result<(), String> {
    if dp.fallback_pathway.trim().is_empty() {
        return Err("decision point must declare a mandatory fallback pathway (§18)".into());
    }
    if candidate_count > MAX_PATHWAYS {
        return Err(format!(
            "decision point exceeds {MAX_PATHWAYS} pathways (§18)"
        ));
    }
    validate_hygiene(&dp.hygiene)?;
    Ok(())
}

pub fn validate_hygiene(hygiene: &Hygiene) -> Result<(), String> {
    if !hygiene.tau.is_finite()
        || !hygiene.delta.is_finite()
        || !hygiene.entropy_ceiling.is_finite()
        || hygiene.delta < 0.0
        || !(0.0..=1.0).contains(&hygiene.entropy_ceiling)
    {
        return Err("pathway hygiene thresholds are invalid".into());
    }
    Ok(())
}

/// Run selection hygiene over the candidate scores. All three checks are ANDed; failing any one is a
/// fallback (never a best-guess). The returned metrics are what the pathway_pick ledger entry
/// records (§12.2).
pub fn select(candidates: &[PathwayScore], hygiene: &Hygiene) -> Selection {
    if candidates.len() > MAX_PATHWAYS {
        return Selection::Fallback {
            reason: FallbackReason::TooManyCandidates,
            entropy: 1.0,
        };
    }
    let mut ids = std::collections::BTreeSet::new();
    if validate_hygiene(hygiene).is_err()
        || candidates
            .iter()
            .any(|candidate| !candidate.score.is_finite() || !ids.insert(&candidate.pathway_id))
    {
        return Selection::Fallback {
            reason: FallbackReason::InvalidInput,
            entropy: 1.0,
        };
    }
    let entropy = normalized_entropy(candidates);
    if candidates.is_empty() {
        return Selection::Fallback {
            reason: FallbackReason::Empty,
            entropy,
        };
    }
    let mut sorted: Vec<&PathwayScore> = candidates.iter().collect();
    sorted.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top = sorted[0].score;
    let second = sorted.get(1).map(|p| p.score).unwrap_or(0.0);
    let margin = top - second;

    if top < hygiene.tau {
        return Selection::Fallback {
            reason: FallbackReason::LowConfidence,
            entropy,
        };
    }
    if margin < hygiene.delta {
        return Selection::Fallback {
            reason: FallbackReason::ThinMargin,
            entropy,
        };
    }
    if entropy > hygiene.entropy_ceiling {
        return Selection::Fallback {
            reason: FallbackReason::HighEntropy,
            entropy,
        };
    }
    Selection::Picked {
        pathway_id: sorted[0].pathway_id.clone(),
        top,
        margin,
        entropy,
    }
}

/// Shannon entropy of the score distribution, normalized to [0,1] by log(n). Scores are treated as
/// non-negative similarities and normalized **directly** (`pᵢ = scoreᵢ / Σ score`) — not through a
/// softmax, whose flatness over small similarity magnitudes would wrongly veto a clear winner. A
/// flat distribution → ~1.0 (maximally uncertain); a peaked one → near 0.
pub fn normalized_entropy(candidates: &[PathwayScore]) -> f64 {
    let n = candidates.len();
    if n <= 1 {
        return 0.0;
    }
    if candidates
        .iter()
        .any(|candidate| !candidate.score.is_finite())
    {
        return 1.0;
    }
    // Shift so the minimum is 0, keeping weights non-negative even if a score is negative.
    let min = candidates
        .iter()
        .map(|c| c.score)
        .fold(f64::INFINITY, f64::min);
    let shift = if min < 0.0 { -min } else { 0.0 };
    let weights: Vec<f64> = candidates.iter().map(|c| c.score + shift).collect();
    let sum: f64 = weights.iter().sum();
    if sum <= 0.0 {
        return 1.0; // all-zero ⇒ maximally uncertain
    }
    let mut h = 0.0;
    for w in &weights {
        let p = w / sum;
        if p > 0.0 {
            h -= p * p.ln();
        }
    }
    h / (n as f64).ln()
}

/// Pathway explainer (§26): the score vector, the τ/δ/entropy checks, and the pick-or-fallback
/// reason — the human-readable form of the `pathway_pick` ledger entry. Pure over the same inputs
/// `select` saw, so a trace viewer can reproduce exactly why a decision point routed where it did.
pub fn explain(candidates: &[PathwayScore], hygiene: &Hygiene) -> String {
    let mut sorted: Vec<&PathwayScore> = candidates.iter().collect();
    sorted.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = String::from("pathway decision (§18)\n");
    for (i, c) in sorted.iter().enumerate() {
        out.push_str(&format!(
            "  {}. {:<20} score={:.3}\n",
            i + 1,
            c.pathway_id,
            c.score
        ));
    }
    let entropy = normalized_entropy(candidates);
    out.push_str(&format!(
        "  hygiene: τ={:.2} δ={:.2} entropy_ceiling={:.2}  |  entropy={:.3}\n",
        hygiene.tau, hygiene.delta, hygiene.entropy_ceiling, entropy
    ));
    match select(candidates, hygiene) {
        Selection::Picked {
            pathway_id,
            top,
            margin,
            entropy,
        } => out.push_str(&format!(
            "  → PICKED {pathway_id} (top={top:.3}, margin={margin:.3}, entropy={entropy:.3})"
        )),
        Selection::Fallback { reason, entropy } => {
            let why = match reason {
                FallbackReason::Empty => "no candidates",
                FallbackReason::LowConfidence => "top score below τ",
                FallbackReason::ThinMargin => "margin below δ",
                FallbackReason::HighEntropy => "distribution too flat (entropy above ceiling)",
                FallbackReason::InvalidInput => "invalid score or hygiene configuration",
                FallbackReason::TooManyCandidates => "candidate cap exceeded",
            };
            out.push_str(&format!("  → FALLBACK ({why}; entropy={entropy:.3})"));
        }
    }
    out
}
