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
pub mod adaptive;
pub mod blocks;
pub mod contract;
pub mod decision_log;
pub mod documents;
pub mod embedding;
pub mod harness;
pub mod memory;
pub mod ops;
pub mod orchestration;
pub mod path;
pub mod policy;
pub mod provider;
pub mod recall;
pub mod reuse_metrics;
pub mod runtime;
pub mod storage;
pub mod tenant;
pub mod types;

pub use adaptive::{
    from_tier_lookup, AbstainReasonV1, AdaptiveArtifactHost, AdaptiveArtifactOutputV1,
    AdaptiveArtifactTurnV1, AdaptiveDecisionEnvelopeV1, AdaptiveDecisionV1, AdaptiveFlowDemandV1,
    ArtifactCandidateV1, ArtifactPinV1, CandidateReasonV1, CapabilityReasonV1, CapabilityRequestV1,
};
pub use contract::{AbilityContract, AbilityPath, FieldSchema, Predicate, TypeSchema};
pub use harness::{
    detect_stack_control, quick_reply_program, select_starter_harness, starter_catalog,
    starter_harness_library, ContextPage, HarnessFrame, HarnessPlayMode, HarnessProgramV1,
    HarnessSession, HarnessStepV1, StackControl, StarterHarness, CONDUCTOR_ID, QUICK_REPLY_ID,
    UNDERSTAND_INTENT_ID, WAIT_FOR_USER_ID,
};
pub use runtime::World;
pub use types::{
    AelioError, AelioResult, CostClass, Depth, LookupTier, ReasonCode, Recovery, Substrate, Value,
};
