//! Refinement gate — typed Evaluate.* verdict and repair loop.

use crate::types::{AelioError, AelioResult, ReasonCode, Value};
use aelio_sol::{EvalFailureReason, EvalVerdict, SlotSpec, TaskNode};
use indexmap::IndexMap;

/// Deterministic shape check against declared outputs.
pub fn evaluate_shape(output: &Value, specs: &[SlotSpec]) -> EvalVerdict {
    if specs.is_empty() {
        return EvalVerdict {
            pass: true,
            confidence: 1.0,
            failure_reason: EvalFailureReason::None,
            repair_hint: String::new(),
        };
    }
    let map = match output {
        Value::Map(m) => m,
        _ => {
            return EvalVerdict {
                pass: false,
                confidence: 0.0,
                failure_reason: EvalFailureReason::ShapeMismatch,
                repair_hint: "Output must be a map with declared fields".into(),
            };
        }
    };
    for spec in specs {
        if !map.contains_key(&spec.name) {
            return EvalVerdict {
                pass: false,
                confidence: 0.0,
                failure_reason: EvalFailureReason::ShapeMismatch,
                repair_hint: format!("Missing required output field `{}`", spec.name),
            };
        }
    }
    EvalVerdict {
        pass: true,
        confidence: 1.0,
        failure_reason: EvalFailureReason::None,
        repair_hint: String::new(),
    }
}

/// Full evaluation: shape first, then optional semantic check hook.
pub fn evaluate_node_output(
    node: &TaskNode,
    output: &Value,
    semantic_check: impl FnOnce(&TaskNode, &Value) -> EvalVerdict,
) -> EvalVerdict {
    let shape = evaluate_shape(output, &node.outputs);
    if !shape.pass {
        return shape;
    }
    semantic_check(node, output)
}

/// Apply refinement loop budget.
pub fn should_refine(node: &TaskNode, refinements_used: u8, verdict: &EvalVerdict) -> bool {
    !verdict.pass && refinements_used < node.max_refinements
}

/// Extract repair context for re-proposal.
pub fn repair_context(node: &TaskNode, verdict: &EvalVerdict, failed_output: &Value) -> String {
    format!(
        "node={} goal={} failure={:?} hint={} failed_output={}",
        node.id,
        node.goal,
        verdict.failure_reason,
        verdict.repair_hint,
        serde_json::to_string(failed_output).unwrap_or_default()
    )
}

/// Join final graph outputs per join spec.
pub fn join_outputs(
    join: &aelio_sol::JoinSpec,
    outputs: &IndexMap<String, Value>,
) -> AelioResult<Value> {
    use aelio_sol::JoinSpec;
    match join {
        JoinSpec::LastNode { node_id, field } => {
            let node_out = outputs.get(node_id).ok_or_else(|| {
                AelioError::new(
                    ReasonCode::Missing,
                    format!("join node `{node_id}` has no output"),
                )
            })?;
            if let Value::Map(map) = node_out {
                map.get(field)
                    .cloned()
                    .ok_or_else(|| {
                        AelioError::new(
                            ReasonCode::Missing,
                            format!("join field `{field}` missing"),
                        )
                    })
            } else {
                Err(AelioError::new(
                    ReasonCode::Validation,
                    "join node output must be a map",
                ))
            }
        }
        JoinSpec::MergeMap { node_fields } => {
            let mut merged = IndexMap::new();
            for (node_id, field) in node_fields {
                if let Some(Value::Map(map)) = outputs.get(node_id) {
                    if let Some(v) = map.get(field) {
                        merged.insert(format!("{node_id}.{field}"), v.clone());
                    }
                }
            }
            Ok(Value::Map(merged))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aelio_sol::TaskBudget;

    #[test]
    fn shape_check_catches_missing_field() {
        let node = TaskNode {
            id: "n".into(),
            goal: "g".into(),
            rationale: "r".into(),
            strategy_hint: vec![],
            inputs: vec![],
            outputs: vec![SlotSpec {
                name: "average".into(),
                ty: "float".into(),
            }],
            depends_on: vec![],
            budget: TaskBudget {
                max_steps: 1,
                max_tokens: 100,
                max_ms: 1000,
            },
            max_refinements: 1,
        };
        let verdict = evaluate_shape(&Value::Map(IndexMap::new()), &node.outputs);
        assert!(!verdict.pass);
    }
}
