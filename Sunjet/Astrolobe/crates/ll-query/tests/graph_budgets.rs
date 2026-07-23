use std::time::Duration;

use ll_query::{
    ColumnKind, Database, DatabaseQueryError, GraphBudget, GraphBudgetKind, HybridQuery,
    QueryError, Value,
};

fn budget(max_frontier: usize) -> GraphBudget {
    GraphBudget {
        max_seeds: 2,
        max_depth: 3,
        max_frontier,
        max_visited: 10,
        max_elapsed: Duration::from_secs(1),
        deadline: None,
    }
}

#[test]
fn graph_budget_failure_is_typed() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path()).unwrap();
    db.create_table("nodes", &[("links", ColumnKind::Edge)])
        .unwrap();
    db.insert("nodes", &[("links", Value::Edges(vec![2, 3]))])
        .unwrap();
    db.insert("nodes", &[("links", Value::Edges(Vec::new()))])
        .unwrap();
    db.insert("nodes", &[("links", Value::Edges(Vec::new()))])
        .unwrap();

    let error = db
        .query_checked(
            "nodes",
            &HybridQuery::new(10)
                .graph("links", vec![1], 1)
                .graph_budget(budget(1)),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        DatabaseQueryError::Execution(QueryError::GraphBudgetExceeded(exceeded))
            if exceeded.kind == GraphBudgetKind::Frontier
    ));
}

#[test]
fn graph_hops_use_newest_edges_and_stay_in_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Database::create(dir.path()).unwrap();
    db.create_table("nodes", &[("links", ColumnKind::Edge)])
        .unwrap();
    db.create_table("other", &[("links", ColumnKind::Edge)])
        .unwrap();
    db.insert("nodes", &[("links", Value::Edges(vec![2]))])
        .unwrap();
    db.insert("nodes", &[("links", Value::Edges(Vec::new()))])
        .unwrap();
    db.insert("other", &[("links", Value::Edges(Vec::new()))])
        .unwrap();
    db.flush().unwrap();

    // Newest node 1 removes its old edge to 2 and points across the table boundary to 3.
    db.update("nodes", 1, &[("links", Value::Edges(vec![3]))])
        .unwrap();
    let hits = db
        .query_checked(
            "nodes",
            &HybridQuery::new(10)
                .graph("links", vec![1], 1)
                .graph_budget(budget(10)),
        )
        .unwrap();
    assert!(hits.is_empty());
}
