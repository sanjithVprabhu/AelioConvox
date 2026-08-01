//! L1 abilities — atomic capabilities with cost and failure modes.

pub mod bind;
pub mod express;
pub mod invoke;
pub mod judge;
pub mod learn;
pub mod registry;
pub mod sense;
pub mod sig;
pub mod state;
pub mod understand;

pub use bind::*;
pub use express::*;
pub use invoke::*;
pub use judge::*;
pub use learn::{
    attribute, bucket_last_seen, bucket_turn_index, compose, cosine_similarity,
    default_situation_embedder, demote, embed_situation, embed_situation_filter, lookup_tier,
    lookup_tier_with_embedder, promote, propose as propose_procedure, propose_path_greeting,
    propose_path_prompt_spec, propose_path_with_provider, situation_hash, situation_key,
    situation_near_text, typecheck, Proposal, ProposalMap, SeenBucket, SituationKey, StepCredit,
    TierLookup, TurnBucket, SITUATION_EMBED_DIM, TIER1_MARGIN_THRESHOLD,
};
pub use registry::*;
pub use sense::*;
pub use sig::{
    classify, compute, extract, hash, match_plan, propose as propose_plan, ExtractField,
    ExtractionPlan, PlanStatus, SignatureRegistry,
};
pub use state::*;
pub use understand::*;
