//! Hit-rate economics & instrumentation (§21, F14). The metrics the dashboard ships with, and the
//! **pre-registered kill criteria** — written before the data exists, so a good number is credible
//! because the bar predates the jump. Adjustment after launch requires a Decision Log entry.

/// Pre-registered kill criteria (§21.2). Placeholders are calibrated + deployer-adjustable
/// pre-launch; post-launch changes need a Decision Log entry.
#[derive(Debug, Clone, Copy)]
pub struct KillCriteria {
    /// Converters (the load-bearing bet): warm-hit < ~60% at 90 days ⇒ conversion-economics claim
    /// falsified; README changes.
    pub converter_warm_hit_floor: f64,
    pub converter_window_days: u32,
    /// Procedures (the speculative bet): < 10% of turns touching a promoted procedure at 6 months
    /// across ≥3 active tenants ⇒ demoted from headline to experimental.
    pub procedure_touch_floor: f64,
    pub procedure_window_days: u32,
    pub procedure_min_tenants: u32,
}

impl Default for KillCriteria {
    fn default() -> Self {
        KillCriteria {
            converter_warm_hit_floor: 0.60,
            converter_window_days: 90,
            procedure_touch_floor: 0.10,
            procedure_window_days: 180,
            procedure_min_tenants: 3,
        }
    }
}

/// Warm-hit metrics for the converter class (§21).
#[derive(Debug, Clone, Copy, Default)]
pub struct ConverterMetrics {
    pub warm_hits: u64,
    pub total_lookups: u64,
    pub days_observed: u32,
}

impl ConverterMetrics {
    pub fn warm_hit_rate(&self) -> f64 {
        if self.total_lookups == 0 {
            0.0
        } else {
            self.warm_hits as f64 / self.total_lookups as f64
        }
    }
}

/// The conversion-economics claim is falsified iff, at/after the window, warm-hit is below the floor.
pub fn converter_falsified(m: &ConverterMetrics, kc: &KillCriteria) -> bool {
    m.days_observed >= kc.converter_window_days && m.warm_hit_rate() < kc.converter_warm_hit_floor
}

/// Procedure-touch metrics (§21).
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcedureMetrics {
    pub turns_touching_a_procedure: u64,
    pub total_turns: u64,
    pub days_observed: u32,
    pub active_tenants: u32,
}

impl ProcedureMetrics {
    pub fn touch_rate(&self) -> f64 {
        if self.total_turns == 0 {
            0.0
        } else {
            self.turns_touching_a_procedure as f64 / self.total_turns as f64
        }
    }
}

/// Procedure learning is demoted from headline to experimental iff, at/after 6 months across ≥3
/// tenants, the touch rate is below the floor.
pub fn procedure_demoted_to_experimental(m: &ProcedureMetrics, kc: &KillCriteria) -> bool {
    m.days_observed >= kc.procedure_window_days
        && m.active_tenants >= kc.procedure_min_tenants
        && m.touch_rate() < kc.procedure_touch_floor
}

/// Fallback rate (§18) — the de-facto drift detector. Rising fallback rate ⇒ distribution shift.
pub fn fallback_rate(fallbacks: u64, decisions: u64) -> f64 {
    if decisions == 0 {
        0.0
    } else {
        fallbacks as f64 / decisions as f64
    }
}
