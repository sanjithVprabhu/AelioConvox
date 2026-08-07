//! # harness-core — HKv4 Starlark substrate (Phase 2, F-033)
//!
//! Deterministic runtime foundation for LLM-generated Starlark modules. Depends on
//! [`aelio_sol`] for §4.3 canonical hashing; this crate adds HKv4 layers the mother doc
//! did not include:
//!
//! - [`numeric`] — Int / Float / Decimal tower, type-strict ops (§2.1–2.2)
//! - [`agg`] — null-explicit aggregates, count_rows/count_values split (§2.3–2.4)
//! - [`taint`] — per-value taint + PC-taint for implicit-flow control (§5.9)
//! - [`versioning`] — SystemVersion, Verified&lt;T&gt;, op impl_hash Merkle root (§17)
//! - [`exec`] — shared budget pool, journal, verify_replay (§3.5, §4.5)
//!
//! Starlark embedding (§8) wires here — not before taint and budget are closed.

#![warn(clippy::disallowed_types)]

mod hash;

pub mod agg;
pub mod exec;
pub mod numeric;
pub mod taint;
pub mod time;
pub mod versioning;

pub use agg::{
    count_rows, count_values, div_int_or_empty, mean_skip_null_decimal, mean_skip_null_float,
    mean_skip_null_int, mean_strict_decimal, mean_strict_float, mean_strict_int,
    mean_zero_null_decimal, mean_zero_null_float, mean_zero_null_int, sum_decimal, sum_float,
    sum_int, sum_num, AggError, AggOutcome, Empty,
};
pub use exec::{
    execute_effect, hash_step_payload, verify_replay, BudgetDimension, BudgetExhausted,
    BudgetFrame, BudgetPool, ContentHash, EffectClass, EffectJournal, EffectOutcome, ExecutionMode,
    JournalEntry, JournalUnderrun, ReplayCursor, ReplayReport, Step,
};
pub use numeric::{
    add, div, division_scale, mul, sub, Decimal, Float, Int, Num, NumError, RoundingMode,
};
pub use taint::{
    check_prohibited, declassify_count, join_taint, propagate_taint, PcTaint, ProhibitedPosition,
    TaintError, TaintedMap, TaintedValue,
};
pub use time::{
    calendar_bucket, interval_hash, now_ms, tzdb_hash, Granularity, IanaTz, Interval, RelativeExpr,
    TemporalBinding, TimeError, TZDB_VERSION,
};
pub use versioning::{
    assert_effect_set_bounds, authorize, harness_op_catalog, impl_hash, op_catalog_merkle_root,
    system_version_from_ops, system_version_with_harness_catalog, AstExceedsEffectSet,
    AuthorizationError, EffectSet, Hash, SystemVersion, Verified, VerifyError,
};

/// Re-export canonical hashing from the zero-dep foundation.
pub use aelio_sol::{canonical_bytes, canonical_string, value_hash};
