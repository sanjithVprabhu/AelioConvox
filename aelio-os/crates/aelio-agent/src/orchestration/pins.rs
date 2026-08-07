//! Sealed Sol pin matching — warm path before LLM decomposition.

use aelio_sol::{JoinSpec, SlotRef, SlotSpec, TaskBudget, TaskGraph, TaskNode};
use regex::Regex;

/// Known workflow pins that bypass dynamic orchestration LLM calls.
pub fn match_workflow_pin(utterance: &str) -> Option<TaskGraph> {
    let normalized = utterance.to_lowercase();
    if looks_like_average(&normalized) {
        return Some(average_task_graph());
    }
    None
}

fn looks_like_average(text: &str) -> bool {
    text.contains("average")
        || text.contains("mean")
        || (text.contains("calculate") && text.contains("of"))
}

/// Extract numeric literals from utterance for average workflows.
pub fn extract_numbers(utterance: &str) -> Vec<f64> {
    let re = Regex::new(r"-?\d+(?:\.\d+)?").expect("number regex");
    re.find_iter(utterance)
        .filter_map(|m| m.as_str().parse().ok())
        .collect()
}

fn average_task_graph() -> TaskGraph {
    TaskGraph {
        root_goal: "Calculate average of provided numbers".into(),
        nodes: vec![
            TaskNode {
                id: "sum".into(),
                goal: "Sum all input values".into(),
                rationale: "Average requires total before division".into(),
                strategy_hint: vec!["std.math.sum".into()],
                inputs: vec![SlotRef::Slot {
                    name: "values".into(),
                }],
                outputs: vec![SlotSpec {
                    name: "total".into(),
                    ty: "float".into(),
                }],
                depends_on: vec![],
                budget: TaskBudget {
                    max_steps: 2,
                    max_tokens: 500,
                    max_ms: 5000,
                },
                max_refinements: 1,
            },
            TaskNode {
                id: "avg".into(),
                goal: "Divide total by count".into(),
                rationale: "Complete average calculation".into(),
                strategy_hint: vec!["std.math.divide".into()],
                inputs: vec![
                    SlotRef::NodeOutput {
                        node_id: "sum".into(),
                        field: "total".into(),
                    },
                    SlotRef::Slot {
                        name: "count".into(),
                    },
                ],
                outputs: vec![SlotSpec {
                    name: "result".into(),
                    ty: "float".into(),
                }],
                depends_on: vec!["sum".into()],
                budget: TaskBudget {
                    max_steps: 2,
                    max_tokens: 500,
                    max_ms: 5000,
                },
                max_refinements: 1,
            },
        ],
        join_spec: JoinSpec::LastNode {
            node_id: "avg".into(),
            field: "result".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_pin_matches() {
        assert!(match_workflow_pin("calculate the average of 10 20 30").is_some());
    }

    #[test]
    fn extract_numbers_works() {
        let nums = extract_numbers("average of 10, 20 and 30");
        assert_eq!(nums, vec![10.0, 20.0, 30.0]);
    }
}
