//! # aelio-convert — conversion graph, closed rule ops, generic gate (§13–§16)
//!
//! The safety core. Conversions are planner-inserted edges (§7); their rules come from the closed,
//! non-computational §14 set (containment theorem, §15); and every learned artifact — converter,
//! pathway, procedure, imprint — passes the one generic gate (§16.5): Structural → Shadow
//! (validate-without-consume) → Canary (bounded consumption + attribution) → Promotion, with
//! reach-based tiers routing the semantic residue to humans (§16.2). The mutation harness (§16.6)
//! publishes the gate's catch rate.

pub mod gate;
pub mod lifecycle;
pub mod mutation;
pub mod rules;

pub use gate::{Digest, Evidence, Reach, Thresholds, Tier};
pub use lifecycle::{transition, Status, Trigger};
pub use mutation::{run_harness, MutationReport};
pub use rules::{any_fabricating, apply_rules, parse_rules, Rule, RuleFail};
