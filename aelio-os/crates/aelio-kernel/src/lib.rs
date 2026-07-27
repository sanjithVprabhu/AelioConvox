//! # aelio-kernel — Planner + Executor + ledger + replay + op catalog (§8–§12, §29)
//!
//! The two-phase interpreter (§29): [`plan::plan`] runs the static checks (a passing plan is
//! executable by construction); [`driver::Instance`] executes it over turns, producing the §12.2
//! ledger; [`driver::replay`] re-runs the same walker as a pure function of that ledger, hard-
//! refusing divergence (§12.3, G2). Depends only on `aelio-sol`.

pub mod bag;
pub mod compute;
pub mod driver;
pub mod error;
pub mod exec;
pub mod instr;
mod json;
pub mod ledger;
pub mod plan;
pub mod registry;
pub mod waves;

pub use driver::{replay, Instance, Parked, TurnOutcome};
pub use error::{ErrV1, ExecResult, ReasonCode};
pub use instr::{parse_node, Node};
pub use ledger::{Category, Ledger};
pub use registry::{EffectClass, Registry};

/// Convert a parsed `serde_json::Value` into a program-free `SolValue` (§4.3 int/float preserved).
/// Exposed for downstream crates (e.g. conversion rule literals).
pub fn json_from(j: &serde_json::Value) -> Result<aelio_sol::SolValue, aelio_sol::SolError> {
    json::from_json(j)
}

/// Parse instruction JSON text into a checked plan (parse + [`plan::plan`]). Convenience for tests
/// and the CLI.
pub fn compile(json_text: &str) -> Result<Node, ErrV1> {
    let value: serde_json::Value = serde_json::from_str(json_text)
        .map_err(|e| ErrV1::new(ReasonCode::Shape, "?", format!("invalid JSON: {e}")))?;
    let node = parse_node(&value)?;
    plan::plan(&node)?;
    Ok(node)
}
