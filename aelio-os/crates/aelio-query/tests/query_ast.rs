//! §10.4 closed query AST — mandatory limit, traverse bounds, tenant isolation.

use aelio_query::{check_admissible, parse_query, Dataset, DatasetRegistry, QOp};

#[test]
fn limit_is_mandatory_and_positive() {
    let err = parse_query(&serde_json::json!({
        "dataset": "clients",
        "qop": "get",
        "params": {}
    }))
    .unwrap_err();
    assert!(err.contains("limit"), "{err}");

    let err = parse_query(&serde_json::json!({
        "dataset": "clients",
        "qop": "get",
        "params": {},
        "limit": 0
    }))
    .unwrap_err();
    assert!(err.contains("limit"), "{err}");
}

#[test]
fn traverse_requires_depth_and_nodes() {
    let err = parse_query(&serde_json::json!({
        "dataset": "graph",
        "qop": "traverse",
        "params": {},
        "limit": 10
    }))
    .unwrap_err();
    assert!(
        err.contains("max_depth") || err.contains("max_nodes"),
        "{err}"
    );

    let q = parse_query(&serde_json::json!({
        "dataset": "graph",
        "qop": "traverse",
        "params": {},
        "limit": 10,
        "max_depth": 3,
        "max_nodes": 100
    }))
    .unwrap();
    assert_eq!(q.qop, QOp::Traverse);
}

#[test]
fn tenant_isolation_on_dataset_lookup() {
    let mut reg = DatasetRegistry::default();
    reg.declare(
        "t1",
        Dataset {
            id: "clients".into(),
            modalities: vec![QOp::Get, QOp::TopkVector],
        },
    );
    let q = parse_query(&serde_json::json!({
        "dataset": "clients",
        "qop": "get",
        "params": {},
        "limit": 5
    }))
    .unwrap();
    assert!(check_admissible(&reg, "t1", &q).is_ok());
    assert!(
        check_admissible(&reg, "t2", &q).is_err(),
        "no cross-tenant fallback"
    );
    let bad = parse_query(&serde_json::json!({
        "dataset": "clients",
        "qop": "topk_bm25",
        "params": {},
        "limit": 5
    }))
    .unwrap();
    assert!(check_admissible(&reg, "t1", &bad).is_err());
}

#[test]
fn query_schema_is_closed_and_bounds_are_hard_capped() {
    assert!(parse_query(&serde_json::json!({
        "dataset": "clients",
        "qop": "get",
        "limit": 10,
        "query_text": "select *"
    }))
    .is_err());
    assert!(parse_query(&serde_json::json!({
        "dataset": "clients",
        "qop": "get",
        "limit": 10,
        "max_depth": 2
    }))
    .is_err());
    assert!(parse_query(&serde_json::json!({
        "dataset": "clients",
        "qop": "get",
        "limit": 10_001
    }))
    .is_err());
    assert!(parse_query(&serde_json::json!({
        "dataset": "graph",
        "qop": "traverse",
        "limit": 10,
        "max_depth": 33,
        "max_nodes": 100
    }))
    .is_err());
}

#[test]
fn no_free_text_qop() {
    assert!(parse_query(&serde_json::json!({
        "dataset": "x",
        "qop": "select * from t",
        "params": {},
        "limit": 1
    }))
    .is_err());
}
