//! # aelio-kernel — Planner + Executor + ledger + replay + op catalog (§8–§12, §29)
//!
//! The two-phase interpreter (§29): [`plan::plan`] runs the static checks (a passing plan is
//! executable by construction); [`driver::Instance`] executes it over turns, producing the §12.2
//! ledger; [`driver::replay`] re-runs the same walker as a pure function of that ledger, hard-
//! refusing divergence (§12.3, G2). Depends only on `aelio-sol`.

pub mod bag;
pub mod call_isa;
pub mod compute;
pub mod conductor_catalog_sim;
pub mod conductor_decide;
pub mod conductor_sol_turn;
pub mod continuation;
pub mod cutover;
pub mod driver;
pub mod error;
pub mod event_admission;
pub mod exec;
pub mod harness_contract;
pub mod harness_syscalls;
pub mod instr;
mod json;
pub mod ledger;
pub mod library_bundle;
pub mod os_contract;
pub mod otp_journey;
pub mod plan;
pub mod process_store;
pub mod process_tree;
pub mod promote;
pub mod registry;
pub mod shadow;
pub mod sol_harness_lib;
pub mod stdlib_targets;
pub mod sugar;
pub mod tool_workflows;
pub mod trace;
pub mod tree_replay;
pub mod waves;

pub use cutover::{
    is_greeting_cutover_utterance, run_greeting_cutover, should_cutover_deterministic,
    should_cutover_greeting, CutoverGreetingResult,
};
pub use conductor_catalog_sim::{
    assert_sim_turn_executable, ensure_sim_catalog_library, grade_sim_turn,
    run_conductor_catalog_sim_turn, run_sim_chat_oracle, run_sim_chat_oracle_diverse,
    run_sim_chat_oracle_script, sim_catalog_contracts, sim_catalog_ids, sim_chat_script,
    sim_chat_script_diverse, sim_choice_matches, sim_conductor_contract, sim_decide_catalog,
    sim_oracle_decide, SimChatExpect, SimTurnGrade,
};
pub use conductor_decide::{
    catalog_ids, decide as conductor_decide, decide_injected, model_decide,
    register_conductor_decide, register_conductor_decide_model, scripted_decide, validate_decision,
    DecideArgs, ModelVerdict, DECIDE_CALL_ID, KIND_QUICK_REPLY, KIND_ROUGH_CHAT, KIND_SPAWN,
};
pub use conductor_sol_turn::{
    demo_conductor_baby_contract, demo_conductor_catalog, demo_sum_ok_contract,
    ensure_demo_conductor_library, run_conductor_baby_turn, should_cutover_conductor_sol,
    ConductorSolTurnResult, DecideMode,
};
pub use driver::{replay, Instance, InstanceConfig, Parked, TurnOutcome};
pub use error::{ErrV1, ExecResult, ReasonCode};
pub use instr::{parse_node, Node};
pub use ledger::{Category, Ledger};
pub use os_contract::{
    ArtifactOriginV1, BudgetContractV1, ChildJoinV1, ConductorActionV1, DeterminismV1,
    EffectClassV1, EventDeliveryV1, EventIdentityV1, HarnessContractV1, InstanceRecordV1,
    InstanceStateV1, JoinPolicyV1, NormalizedEventV1, ResultEnvelopeV1, SpawnChildV1,
    SuspendabilityV1, VersionPinV1,
};
pub use otp_journey::{
    begin_otp_session_after_send, looks_like_otp_code, otp_login_program_json,
    otp_login_resume_with_code, otp_login_start_until_park, otp_session_clear, otp_session_get,
    try_complete_otp_session, OtpSessionV1, OTP_SESSION_TABLE,
};
pub use event_admission::{
    admit_event, event_from_user_utterance, AdmitResult, AdmitStatus, ConductorAdmitOutcome,
    EVENT_DEDUPE_TABLE, EVENT_TABLE,
};
pub use harness_syscalls::{
    collection_count_program_json, conductor_root_program_json, decide_deterministic,
    math_divide_program_json, math_sum_program_json, run_conductor_root_deterministic,
    run_pure_sol, workflow_average_via_join, DeterministicConductorDecision,
};
pub use library_bundle::{
    install_from_manifest, install_seed_sol_library, load_installed_hash, resolve_library_root,
    InstallOutcome, InstallReport,
};
pub use process_store::{
    ProcessRepository, BUDGET_TABLE, INSTANCE_TABLE, JOIN_TABLE, WAIT_TABLE,
};
pub use process_tree::{InstanceBudgetV1, ProcessError, ProcessTree, WaitPredicate};
pub use promote::{
    admin_promote, draft_from_contract, load_promoted_program, sandbox_draft, save_draft,
    DraftStatus, HarnessDraftV1, PromoteOutcome, PromoteReport, DRAFT_TABLE, PROMOTE_AUDIT_TABLE,
};
pub use registry::{EffectClass, Registry};
pub use shadow::{
    classify_agent_steps, observe_shadow, shadow_trace_detail, store_shadow, ShadowObservation,
    ShadowRoute, SHADOW_TABLE,
};
pub use tool_workflows::{
    extract_phone, resolve_tool_harness_intent, run_confirm_then_act, run_installed_harness,
    run_memory_attach, run_send_otp, tool_bag_reply_text, try_run_tool_intent, ToolHarnessIntent,
};
pub use tree_replay::{
    replay_tree, run_pure_sol_recorded, workflow_average_recorded, ChildExecutionRecord,
    TreeExecutionRecord, TreeReplayReport,
};
pub use call_isa::{
    catalog_from_seed_library, frozen_admission_registry, frozen_call_ids, frozen_spec,
    is_allowed_call_id, register_frozen_call_isa, seed_prompt_pins, CallSpec, CatalogEntry,
    PromptArtifactPin, FROZEN_CALL_SPECS, HARNESS_CALL_MAX_DEPTH, LLM_FAMILY, MEMORY_FAMILY,
    PROC_FAMILY, TOOL_FAMILY,
};
pub use harness_contract::{
    admit_contract, collect_call_ids, contract_identity, effect_from_str, effect_to_str,
    infer_effect_from_calls, library_from_id, BudgetSpec, HarnessSignature, SlotSpec,
};
pub use sol_harness_lib::{
    calc_sum_gate_contract, greet_then_offer_help_contract, leaf_call_registry,
    library_program_map, list_contract_ids, load_contract, memory_attach_contract,
    parent_waits_on_child_contract, proper_sol_stack_leaf_c_contract,
    proper_sol_stack_mid_b_contract, proper_sol_stack_program_map,
    proper_sol_stack_top_a_contract, quick_reply_contract, registry_with_harness_invoke,
    registry_with_proper_stack_flows, seed_admission_registry, semantic_ack_contract,
    sol_harness_library, stack_leaf_c_contract, stack_mid_b_contract, stack_top_a_contract,
    store_library, store_library_admitted, understand_intent_contract, wait_for_user_contract,
    workflow_average_contract, workflow_average_program_json, SolHarnessContract, CONTRACT_TABLE,
    FLOW_STACK_LEAF_C, FLOW_STACK_MID_B, FLOW_STACK_TOP_A, HARNESS_INVOKE_MAX_DEPTH,
};
pub use stdlib_targets::{register_p0_pure_stdlib, registry_with_p0_stdlib};

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
