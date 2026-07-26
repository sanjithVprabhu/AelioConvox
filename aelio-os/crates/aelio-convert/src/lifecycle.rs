//! Artifact lifecycle state machine (App K, §13.1). Generic across artifact classes (§16.5) — only
//! the agreement predicate differs; the transitions here are shared. Demotion is an *event* whose
//! target is `Shadow` (or `Canary` for kernel-bump demotions, §25.5).

/// Lifecycle states (App K). `Demoted` is not a state — it is an event with a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Proposed,
    Rejected,
    Shadow,
    Canary,
    Promoted,
    Retired,
}

/// Reviewed-tier consumption gate: whether the deployer approved (App K refinement — approval gates
/// first *consumption*, i.e. shadow→canary, not promotion).
#[derive(Debug, Clone, Copy)]
pub enum Trigger {
    StructuralPass,
    StructuralFail,
    /// distinct_inputs ≥ 20 ∧ validation_rate ≥ 0.95 (§16.4). `approved` = reviewed-tier deployer
    /// approval; auto tier passes `true` trivially.
    ShadowThresholdsMet { approved: bool },
    /// distinct canary inputs ≥ 20 ∧ downstream_success ≥ 0.98 ∧ zero attributed Guard.Violation.
    CanaryThresholdsMet,
    /// Attributed failure > 2% trailing ∨ any attributed Guard.Violation (§16.4).
    AttributedFailure,
    /// Imprint version bump (§4.1.8) / flow edit (§4.1.6) / evidence-dependent template bump.
    Invalidated,
    /// Kernel version bump ⇒ canary-all (§25.5).
    KernelBump,
    RetireManual,
    ThreeDemotions,
    UnusedWindow,
}

/// Apply the App K transition table. `Err` = no legal transition for this (status, trigger).
pub fn transition(from: Status, trigger: Trigger) -> Result<Status, &'static str> {
    use Status::*;
    use Trigger::*;
    Ok(match (from, trigger) {
        (Proposed, StructuralPass) => Shadow,
        (Proposed, StructuralFail) => Rejected,
        (Shadow, ShadowThresholdsMet { approved }) => {
            if approved {
                Canary
            } else {
                return Err("reviewed tier: deployer approval gates first consumption (App K)");
            }
        }
        (Canary, CanaryThresholdsMet) => Promoted,
        // demotions (event → shadow)
        (Canary | Promoted, AttributedFailure) => Shadow,
        (Promoted, Invalidated) => Shadow,
        // kernel bump: promoted → canary (canary-all)
        (Promoted, KernelBump) => Canary,
        // retirement
        (Proposed | Shadow | Canary | Promoted, RetireManual | ThreeDemotions | UnusedWindow) => Retired,
        // revival
        (Retired, StructuralPass) => Shadow,
        _ => return Err("illegal transition (App K)"),
    })
}
