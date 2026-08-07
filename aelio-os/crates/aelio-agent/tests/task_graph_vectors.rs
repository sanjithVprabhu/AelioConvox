//! Conformance vectors for TaskGraph validation (Harness v2).

use aelio_sol::{TaskBudget, TaskGraph, TaskGraphValidationError};

#[test]
fn vector_valid_linear() {
    let raw = include_str!("../../../../docs/vectors/task_graph_valid_linear.json");
    let parsed: serde_json::Value = serde_json::from_str(raw).expect("json");
    let graph: TaskGraph = serde_json::from_value(parsed["graph"].clone()).expect("graph");
    assert!(graph.validate(&TaskBudget::default_turn()).is_ok());
}

#[test]
fn vector_cycle_reject() {
    let raw = include_str!("../../../../docs/vectors/task_graph_cycle_reject.json");
    let parsed: serde_json::Value = serde_json::from_str(raw).expect("json");
    let graph: TaskGraph = serde_json::from_value(parsed["graph"].clone()).expect("graph");
    assert!(matches!(
        graph.validate(&TaskBudget::default_turn()),
        Err(TaskGraphValidationError::CycleDetected)
    ));
}

#[test]
fn vector_budget_overrun_reject() {
    let raw = include_str!("../../../../docs/vectors/task_graph_budget_overrun_reject.json");
    let parsed: serde_json::Value = serde_json::from_str(raw).expect("json");
    let graph: TaskGraph = serde_json::from_value(parsed["graph"].clone()).expect("graph");
    let parent: TaskBudget = serde_json::from_value(parsed["parent_budget"].clone()).expect("budget");
    assert!(matches!(
        graph.validate(&parent),
        Err(TaskGraphValidationError::BudgetOverrun { .. })
    ));
}
