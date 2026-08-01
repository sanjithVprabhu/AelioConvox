//! # Aelio — typed ability runtime for conversational product surfaces
//!
//! Implements the architecture in `docs/new_arch/`:
//! - L0 substrates: combinators (S0-A), pure ops (S0-B), effects (S0-C)
//! - L1 abilities: Sense, Understand, Judge, Bind, Invoke, Sig, Express, State, Policy, Registry, Learn
//! - L2 blocks: turn spine, tool call, flow control, term resolution
//! - L3 procedures (learned) + L4 flows (authored) + L5 lifecycle
//!
//! Core bet: **retrieval and selection are deterministic; the LLM proposes paths
//! over a typed ability set and synthesizes terminal speech.**

pub mod abilities;
pub mod blocks;
pub mod contract;
pub mod decision_log;
pub mod documents;
pub mod embedding;
pub mod memory;
pub mod ops;
pub mod path;
pub mod policy;
pub mod provider;
pub mod recall;
pub mod runtime;
pub mod storage;
pub mod tenant;
pub mod types;

pub use contract::{AbilityContract, AbilityPath, FieldSchema, Predicate, TypeSchema};
pub use runtime::World;
pub use types::{
    AelioError, AelioResult, CostClass, Depth, LookupTier, ReasonCode, Recovery, Substrate, Value,
};
