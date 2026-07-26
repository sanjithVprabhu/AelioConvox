//! Credit assignment / attribution (§20) — **deliberately dumb, honest, conservative**. The flow-
//! outcome boolean is attributed to every artifact used, usage-weighted. Documented biases (popular
//! artifacts accumulate noise; co-occurring artifacts share blame) are *conservative*: they produce
//! false **demotions** (cheap, per the asymmetry), never false **promotions** (expensive). Clever is
//! v2, after data.

/// Demotion-eligible negative signals (§20). Implicit correction detection (rephrasing) is v2 — in
/// v0 it is noise and is deliberately absent from this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegativeSignal {
    /// An error whose `cause` chain passes through the artifact's consumer nid.
    CauseChainError,
    GuardViolation,
    /// Flow abandonment within N turns of artifact use.
    Abandonment,
    /// An **explicit** correction signal only.
    ExplicitCorrection,
}

/// All represented negative signals are demotion-eligible (§20). (There is no "implicit correction"
/// variant by design.)
pub fn is_demotion_eligible(_signal: NegativeSignal) -> bool {
    true
}

/// One artifact's attributed credit for a flow.
#[derive(Debug, Clone, PartialEq)]
pub struct Credit {
    pub artifact_id: String,
    /// 1.0 on flow success, 0.0 on failure — usage-weighted, spread over every artifact used.
    pub credit: f64,
    pub uses: u64,
}

/// Attribute a flow's boolean outcome to every artifact used, weighted by how many times each was
/// used in the flow. Same outcome to all — the conservative v0 scheme (§20).
pub fn attribute(success: bool, artifacts_used: &[(String, u64)]) -> Vec<Credit> {
    let outcome = if success { 1.0 } else { 0.0 };
    artifacts_used
        .iter()
        .map(|(id, uses)| Credit {
            artifact_id: id.clone(),
            credit: outcome * (*uses as f64),
            uses: *uses,
        })
        .collect()
}
