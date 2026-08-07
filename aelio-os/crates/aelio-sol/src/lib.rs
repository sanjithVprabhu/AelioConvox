//! # aelio-sol — Sol Contracts (§4–§6)
//!
//! The zero-internal-dep foundation of the Aelio kernel: the materialized value model
//! ([`SolValue`]), canonical serialization + BLAKE3 hashing (§4.3), §4.4 limits, structural
//! imprints (§4.1.3), and the §6.1 literal path grammar.
//!
//! Load-bearing invariants proven here (P0 step 1 exit, §30):
//! - **`2` (int) ≢ `2.0` (float)** — canonical forms and hashes differ (§4.3).
//! - **`NaN`/`±∞` are unconstructible** — rejected at [`SolValue::float`] (§4.3).
//! - **Canonical hashing is stable** — key order independent; round-trip identical (§4.3, §27).
//! - **Program-bearing bags can't be hashed** — `fn`/`flow` aren't representable in [`SolValue`]
//!   (§4.1.4), so the boundary rule holds by construction.

// Determinism hygiene (F-032): `HashMap`/`HashSet` iteration order is randomized per process.
// Banned crate-wide via `clippy.toml`'s `disallowed-types`; this enables the (allow-by-default)
// lint that config feeds.
#![warn(clippy::disallowed_types)]

mod canonical;
mod error;
mod hash;
mod imprint;
mod limits;
mod path;
mod task_graph;
mod value;

pub use canonical::{to_bytes as canonical_bytes, to_string as canonical_string};
pub use error::{LimitError, SolError, SolResult};
pub use hash::{blake3_hex, value_hash};
pub use imprint::{shape_signature, structural as structural_imprint};
pub use limits::Limits;
pub use path::{Path, Segment};
pub use task_graph::{
    ComplexityClass, EvalFailureReason, EvalVerdict, JoinSpec, SlotRef, SlotSpec, TaskBudget,
    TaskGraph, TaskGraphValidationError, TaskNode,
};
pub use value::{SolValue, TypeTag};
