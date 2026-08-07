//! Tool routing — client registry first, then `std.*`, ephemeral last.
//!
//! Client-registered tools must never be shadowed by Aelio standard or ephemeral fallbacks.

use crate::abilities::registry::Registry;
use crate::orchestration::pins::extract_numbers;
use crate::orchestration::stdlib::{is_standard_tool, STANDARD_PROVIDER};
use crate::tenant::ToolSpec;
use aelio_sol::{JoinSpec, SlotRef, SlotSpec, TaskBudget, TaskGraph, TaskNode};
use indexmap::IndexMap;

/// Where a resolved tool came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRoute {
    Client,
    Standard,
    Ephemeral,
}

/// Returns true for first-party `std.*` tools registered by Aelio.
pub fn is_standard_registry_tool(tool: &ToolSpec) -> bool {
    is_standard_tool(&tool.id)
}

/// Tenant/client tool — anything in the registry that is not `std.*`.
pub fn is_client_tool(tool: &ToolSpec) -> bool {
    !is_standard_registry_tool(tool)
}

/// Summarize the registry for ephemeral context (client + std ids/tags only).
pub fn tool_catalog_for_context(registry: &Registry) -> IndexMap<String, Value> {
    let mut client = Vec::new();
    let mut standard = Vec::new();
    for tool in registry.tools.values() {
        let entry = format!(
            "{} tags=[{}]",
            tool.id,
            tool.capability_tags.join(",")
        );
        if is_client_tool(tool) {
            client.push(entry);
        } else {
            standard.push(entry);
        }
    }
    let mut map = IndexMap::new();
    map.insert("client_tools".into(), Value::List(client.into_iter().map(Value::Str).collect()));
    map.insert(
        "standard_tools".into(),
        Value::List(standard.into_iter().map(Value::Str).collect()),
    );
    map.insert("standard_provider".into(), Value::Str(STANDARD_PROVIDER.into()));
    map
}

/// Resolve a TaskNode hint to a tool with strict priority: client → std → ephemeral (explicit only).
pub fn resolve_tool_for_node(node: &TaskNode, registry: &Registry) -> Result<ToolSpec, String> {
    for hint in &node.strategy_hint {
        if hint.contains("ephemeral") {
            if let Some(tool) = registry.tools.get("std.ephemeral.adapt") {
                return Ok(tool.clone());
            }
            continue;
        }

        // Exact id — client or std equally by registration order (client typically registered after boot std).
        if let Some(tool) = registry.tools.get(hint) {
            return Ok(tool.clone());
        }

        // Capability tag — prefer client tools over std.*
        if let Some(tool) = find_client_tool_by_capability(registry, hint) {
            return Ok(tool);
        }

        let std_id = if hint.starts_with("std.") {
            hint.clone()
        } else {
            format!("std.{hint}")
        };
        if let Some(tool) = registry.tools.get(&std_id) {
            return Ok(tool.clone());
        }
        if let Some(tool) = find_std_tool_by_capability(registry, hint) {
            return Ok(tool);
        }
    }

    Err(format!("no tool for node `{}`", node.id))
}

pub fn route_for_tool(tool: &ToolSpec) -> ToolRoute {
    if tool.id.starts_with("std.ephemeral.") {
        ToolRoute::Ephemeral
    } else if is_standard_registry_tool(tool) {
        ToolRoute::Standard
    } else {
        ToolRoute::Client
    }
}

fn find_client_tool_by_capability(registry: &Registry, tag: &str) -> Option<ToolSpec> {
    registry
        .by_capability
        .get(tag)
        .and_then(|ids| {
            ids.iter()
                .find_map(|id| registry.tools.get(id))
                .filter(|t| is_client_tool(t))
        })
        .cloned()
}

fn find_std_tool_by_capability(registry: &Registry, tag: &str) -> Option<ToolSpec> {
    registry
        .by_capability
        .get(tag)
        .and_then(|ids| {
            ids.iter()
                .find_map(|id| registry.tools.get(id))
                .filter(|t| is_standard_registry_tool(t))
        })
        .cloned()
}

/// Match a std tool when the utterance clearly describes a pure local operation.
///
/// Matching is on whole words, not substrings: "summarize" must not route to `std.math.sum`, nor
/// "meaning" to `std.math.average`. Ambiguous single words ("total", bare "mean") are excluded
/// entirely — a miss costs one fall-through to ProposePath, a false hit answers the wrong question.
pub fn match_std_tool(utterance: &str, registry: &Registry) -> Option<ToolSpec> {
    let lower = utterance.to_lowercase();
    let candidates: &[(&[&str], &str)] = &[
        (&["average"], "std.math.average"),
        (&["sum", "add up"], "std.math.sum"),
        (&["how many", "count items", "count the"], "std.data.count"),
        (&["uppercase", "upper case"], "std.text.upper"),
        (&["lowercase", "lower case"], "std.text.lower"),
        (&["trim", "strip"], "std.text.strip"),
        (&["validate email"], "std.validate.email"),
    ];
    for (keywords, tool_id) in candidates {
        if keywords.iter().any(|kw| contains_phrase(&lower, kw)) {
            if let Some(tool) = registry.tools.get(*tool_id) {
                return Some(tool.clone());
            }
        }
    }
    None
}

/// Whole-word/phrase containment: the match must not be flanked by alphanumerics on either side.
fn contains_phrase(haystack: &str, phrase: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = haystack[from..].find(phrase) {
        let start = from + offset;
        let end = start + phrase.len();
        let before_ok = start == 0 || !(bytes[start - 1] as char).is_alphanumeric();
        let after_ok = end == bytes.len() || !(bytes[end] as char).is_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// Queries that look like external/client-side effects — orchestration must not intercept.
pub fn looks_like_client_effect(utterance: &str) -> bool {
    let lower = utterance.to_lowercase();
    [
        "send ", "book ", "pay ", "purchase", "create ", "delete ", "update ", "fetch ",
        "call ", "invoke ", "register", "login", "verify", "submit", "cancel order",
        "charge", "refund", "notify", "email me", "sms", "otp", "api ",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

/// Pure local tasks solvable without client SDK — eligible for ephemeral sub-agent.
pub fn is_ephemeral_eligible(utterance: &str, has_document: bool) -> bool {
    if looks_like_client_effect(utterance) {
        return false;
    }
    let lower = utterance.to_lowercase();
    if has_document {
        return true;
    }
    if !extract_numbers(utterance).is_empty() {
        return true;
    }
    [
        "calculate", "average", "sum", "count", "how many", "length", "words in",
        "uppercase", "lowercase", "trim", "strip", "concat", "join", "split",
        "compare", "percent", "clamp",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

pub fn single_tool_graph(utterance: &str, tool: &ToolSpec, route: ToolRoute) -> TaskGraph {
    let rationale = match route {
        ToolRoute::Client => "Matched client-registered tool from utterance".into(),
        ToolRoute::Standard => "Matched Aelio standard tool from utterance".into(),
        ToolRoute::Ephemeral => "Runtime ephemeral sub-agent".into(),
    };
    let output_field = tool
        .output_semantics
        .fields
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| "result".into());
    TaskGraph {
        root_goal: format!("Fulfill: {utterance}"),
        nodes: vec![TaskNode {
            id: "main".into(),
            goal: utterance.to_string(),
            rationale,
            strategy_hint: vec![tool.id.clone()],
            inputs: vec![SlotRef::Slot {
                name: "context".into(),
            }],
            outputs: vec![SlotSpec {
                name: output_field.clone(),
                ty: "any".into(),
            }],
            depends_on: vec![],
            budget: TaskBudget {
                max_steps: 4,
                max_tokens: 2000,
                max_ms: 15_000,
            },
            max_refinements: 0,
        }],
        join_spec: JoinSpec::LastNode {
            node_id: "main".into(),
            field: output_field,
        },
    }
}

pub fn propose_ephemeral_graph(utterance: &str) -> TaskGraph {
    TaskGraph {
        root_goal: format!("Fulfill: {utterance}"),
        nodes: vec![TaskNode {
            id: "ephemeral".into(),
            goal: utterance.to_string(),
            rationale: "No client or std tool matched; runtime ephemeral sub-agent".into(),
            strategy_hint: vec!["std.ephemeral.adapt".into()],
            inputs: vec![
                SlotRef::Slot { name: "goal".into() },
                SlotRef::Slot { name: "context".into() },
            ],
            outputs: vec![SlotSpec {
                name: "result".into(),
                ty: "any".into(),
            }],
            depends_on: vec![],
            budget: TaskBudget {
                max_steps: 4,
                max_tokens: 2000,
                max_ms: 15_000,
            },
            max_refinements: 0,
        }],
        join_spec: JoinSpec::LastNode {
            node_id: "ephemeral".into(),
            field: "result".into(),
        },
    }
}

use crate::types::Value;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::stdlib::register_standard_tools;
    use crate::tenant::{OutputField, OutputSpec, ParamSource, ParamSpec, ToolEffect, ToolSpec};
    use crate::types::Sensitivity;

    fn client_tool(id: &str, tags: &[&str]) -> ToolSpec {
        ToolSpec {
            id: id.into(),
            name: id.into(),
            version: "1".into(),
            capability_tags: tags.iter().map(|t| (*t).into()).collect(),
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
            params: vec![ParamSpec {
                name: "phone".into(),
                type_name: "string".into(),
                required: true,
                constraint: None,
                source: ParamSource::Slot { name: "phone".into() },
                repair: None,
                prompt_hint: None,
                sensitivity: Sensitivity::None,
                default: None,
                depends_on: vec![],
            }],
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
    fn client_tool_preferred_over_std_for_capability() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        registry
            .register_tool(client_tool("send_otp", &["auth.otp.send"]))
            .unwrap();
        let node = TaskNode {
            id: "n".into(),
            goal: "send".into(),
            rationale: "r".into(),
            strategy_hint: vec!["auth.otp.send".into()],
            inputs: vec![],
            outputs: vec![],
            depends_on: vec![],
            budget: TaskBudget::default_turn(),
            max_refinements: 0,
        };
        let tool = resolve_tool_for_node(&node, &registry).expect("resolve");
        assert_eq!(tool.id, "send_otp");
        assert_eq!(route_for_tool(&tool), ToolRoute::Client);
    }

    #[test]
    fn effectful_composite_does_not_use_ephemeral() {
        assert!(!is_ephemeral_eligible("send otp and verify account", false));
        assert!(looks_like_client_effect("book a flight and pay"));
    }

    #[test]
    fn std_matching_is_word_bounded() {
        let mut registry = Registry::default();
        register_standard_tools(&mut registry);
        assert!(
            match_std_tool("summarize this thread", &registry).is_none(),
            "`summarize` must not match the `sum` keyword"
        );
        assert!(
            match_std_tool("what is the meaning of this", &registry).is_none(),
            "`meaning` must not match an average keyword"
        );
        assert_eq!(
            match_std_tool("what is the average of these", &registry).map(|t| t.id),
            Some("std.math.average".to_string())
        );
        assert_eq!(
            match_std_tool("sum these numbers", &registry).map(|t| t.id),
            Some("std.math.sum".to_string())
        );
    }
}
