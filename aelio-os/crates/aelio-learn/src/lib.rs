//! # aelio-learn — pathways, procedures, attribution, metrics (§18–§21)
//!
//! Selection is a classifier with hygiene (§18); procedure learning is registration (§19); credit
//! assignment is the conservative v0 scheme (§20). The generic gate (`aelio-convert::gate`) supplies
//! the promotion machinery — this crate supplies only the per-class agreement predicate + selection.

pub mod agreement;
pub mod attribution;
pub mod metrics;
pub mod pathways;
pub mod procedures;

pub use agreement::{pathway_agreement, procedure_agreement, PathwayTurn};
pub use attribution::{attribute, is_demotion_eligible, Credit, NegativeSignal};
pub use metrics::{
    converter_falsified, fallback_rate, procedure_demoted_to_experimental, ConverterMetrics,
    KillCriteria, ProcedureMetrics,
};
pub use pathways::{
    normalized_entropy, select, validate_decision_point, DecisionPoint, FallbackReason, Hygiene,
    PathwayScore, Selection, MAX_PATHWAYS,
};
pub use procedures::{mine, FlowTrace, MiningParams, ProcedureCandidate, TraceStep};
