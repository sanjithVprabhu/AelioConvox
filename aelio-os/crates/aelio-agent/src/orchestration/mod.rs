//! Harness v2 — multi-agent orchestration layer (F-027).

pub mod ephemeral;
pub mod evaluate;
pub mod executor;
pub mod orchestrate;
pub mod pins;
pub mod promotion;
pub mod recursion_guard;
pub mod stdlib;
pub mod suspension;
pub mod tool_router;
pub mod wavefront;

pub use ephemeral::{adapt_with_scope, EphemeralPipeline, EphemeralScope};
pub use evaluate::{evaluate_node_output, evaluate_shape, join_outputs, should_refine};
pub use executor::{execute_task_graph, execute_task_graph_from_state, GraphExecution, GraphExecutionResult};
pub use orchestrate::{classify_complexity, propose_ephemeral_fallback, propose_task_graph};
pub use pins::{extract_numbers, match_workflow_pin};
pub use promotion::observe_task_graph_success;
pub use recursion_guard::RecursionGuard;
pub use stdlib::{invoke_standard_tool, is_standard_tool, register_standard_tools, STANDARD_PROVIDER};
pub use tool_router::{
    is_client_tool, is_ephemeral_eligible, resolve_tool_for_node, route_for_tool, ToolRoute,
};
pub use wavefront::{
    hash_args, next_wave, ExecutorState, LedgerEntry, LedgerStatus, NodeInvocation,
};
