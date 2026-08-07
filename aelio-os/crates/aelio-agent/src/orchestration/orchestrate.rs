//! Complexity classification and TaskGraph construction.

use crate::abilities::registry::Registry;
use crate::orchestration::pins::{extract_numbers, match_workflow_pin};
use crate::orchestration::tool_router::{
    is_ephemeral_eligible, match_std_tool, propose_ephemeral_graph, single_tool_graph, ToolRoute,
};
use aelio_sol::{ComplexityClass, TaskGraph};
use indexmap::IndexMap;

/// Classify utterance complexity for orchestration routing.
///
/// A conjunction alone is not a composite signal — " and " appears in most ordinary sentences,
/// and classifying those as composite hands every conversational turn to the orchestrator before
/// Conductor gets a chance. A conjunction counts only when the utterance also carries something
/// orchestration can actually act on: a computable verb or two or more numeric operands.
pub fn classify_complexity(utterance: &str) -> ComplexityClass {
    let lower = utterance.to_lowercase();
    if match_workflow_pin(utterance).is_some() {
        return ComplexityClass::Composite;
    }
    let numbers = extract_numbers(utterance).len();
    let computable = COMPUTABLE_VERBS.iter().any(|kw| lower.contains(kw));
    let sequenced = lower.contains(" then ");
    let conjoined = lower.contains(" and ");

    if numbers >= 2
        || computable
        || sequenced
        || (conjoined && (numbers >= 1 || computable))
    {
        ComplexityClass::Composite
    } else if lower.split_whitespace().count() > 20 {
        ComplexityClass::Open
    } else {
        ComplexityClass::Atomic
    }
}

/// Verbs that name an operation the std library or an ephemeral pipeline can actually perform.
const COMPUTABLE_VERBS: &[&str] = &[
    "calculate",
    "compute",
    "average",
    "sum of",
    "add up",
    "how many",
    "count the",
    "uppercase",
    "lowercase",
    "percentage",
    "percent of",
];

fn has_document_context(extra_slots: &IndexMap<String, crate::types::Value>) -> bool {
    ["document_text", "document", "attached_text"]
        .iter()
        .any(|k| extra_slots.get(*k).is_some())
}

/// Build or match a TaskGraph. Returns `None` when the query should fall through to Conductor and
/// cold ProposePath.
///
/// Orchestration never guesses a client-registered tool from utterance keywords: selecting a
/// tenant tool is ProposePath's job, where the proposal is typechecked and every call goes through
/// the policy/bind/redaction gate. A client tool reaches this executor only when a verified
/// TaskGraph node names it explicitly in `strategy_hint`.
pub fn propose_task_graph(
    utterance: &str,
    complexity: ComplexityClass,
    registry: &Registry,
    extra_slots: &IndexMap<String, crate::types::Value>,
) -> Option<TaskGraph> {
    match complexity {
        ComplexityClass::Atomic => None,
        ComplexityClass::Composite | ComplexityClass::Open => {
            // 1. Pinned std workflows (e.g. workflow.average)
            if let Some(pin) = match_workflow_pin(utterance) {
                return Some(pin);
            }
            // 2. Unambiguous std tool for a pure local operation
            if let Some(std_tool) = match_std_tool(utterance, registry) {
                return Some(single_tool_graph(utterance, &std_tool, ToolRoute::Standard));
            }
            // 3. Ephemeral sub-agent — pure local tasks only
            if is_ephemeral_eligible(utterance, has_document_context(extra_slots)) {
                return Some(propose_ephemeral_graph(utterance));
            }
            // 4. Fall through to Conductor, then cold ProposePath over declared abilities
            None
        }
    }
}

/// Back-compat alias used in tests.
pub fn propose_ephemeral_fallback(utterance: &str) -> TaskGraph {
    propose_ephemeral_graph(utterance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::stdlib::register_standard_tools;
    use crate::tenant::{OutputField, OutputSpec, ToolEffect, ToolSpec};
    use crate::types::Sensitivity;

    fn client_tool(id: &str) -> ToolSpec {
        ToolSpec {
            id: id.into(),
            name: id.into(),
            version: "1".into(),
            capability_tags: vec![id.into()],
            contract: Some(crate::tenant::ToolContract {
                effect_class: crate::tenant::EffectClass::Write,
                completeness: crate::tenant::Completeness::Complete,
                returns_entity: "client_receipt".into(),
                pushdown: vec![],
                max_result_rows: Some(1),
                row_scoped: false,
            }),
            effect: Some(ToolEffect::External),
            effectful: true,
            idempotent: false,
            dry_run_available: false,
            params: vec![],
            output_semantics: OutputSpec {
                fields: IndexMap::from([(
                    "ok".into(),
                    OutputField {
                        path: "ok".into(),
                        type_name: "bool".into(),
                        sensitivity: Sensitivity::None,
                        meaning: "ok".into(),
                    },
                )]),
                role_hint: None,
            },
            continuations: vec![],
            errors: vec![],
        }
    }

    #[test]
    fn composite_for_average_query() {
        assert_eq!(
            classify_complexity("what is the average of 1 2 3"),
            ComplexityClass::Composite
        );
    }

    #[test]
    fn client_effect_falls_through_to_propose_path() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        let graph = propose_task_graph(
            "send otp and verify account",
            ComplexityClass::Composite,
            &registry,
            &IndexMap::new(),
        );
        assert!(graph.is_none(), "must not intercept client-effect queries");
    }

    /// Orchestration must never select a tenant tool from utterance keywords — even when the
    /// tool id appears verbatim. Tenant tool selection belongs to ProposePath, which typechecks
    /// the proposal and routes the call through the policy/bind/redaction gate.
    #[test]
    fn client_tool_is_never_auto_selected_from_keywords() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry.register_tool(client_tool("send_otp")).unwrap();
        let graph = propose_task_graph(
            "please send_otp to my phone and confirm",
            ComplexityClass::Composite,
            &registry,
            &IndexMap::new(),
        );
        assert!(
            graph.is_none(),
            "client tool selection must fall through to ProposePath, got {graph:?}"
        );
    }

    /// A conversational composite mentioning a capability-tag word ("send") must not be routed
    /// into an effectful tenant tool.
    #[test]
    fn capability_word_in_prose_does_not_reach_client_tool() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry.register_tool(client_tool("send_otp")).unwrap();
        let graph = propose_task_graph(
            "can you send me the report and summarize it",
            ComplexityClass::Composite,
            &registry,
            &IndexMap::new(),
        );
        assert!(graph.is_none(), "prose must not trigger an effectful tool");
    }
}
