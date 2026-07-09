//! `ll-cost` — the LL cost model.
//!
//! Two pieces today: an equi-height [`Histogram`] for scalar selectivity estimation, and
//! a filtered-vector [`Strategy`] selector that turns an estimated selectivity into the
//! cheapest strategy expected to meet a recall target. Together they make the
//! filtered-vector benchmark's finding *automatic*: estimate selectivity → predict costs →
//! choose. Vector cardinality via per-file centroid distance distributions and the full
//! microsecond-calibrated operator cost model build on this foundation.

mod histogram;
mod strategy;
mod vector_stats;

pub use histogram::Histogram;
pub use strategy::{choose, estimate_all, CostEstimate, Strategy, VecCostParams};
pub use vector_stats::VectorStats;
