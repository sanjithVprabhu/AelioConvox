//! §10.4 query-AST conformance: mandatory limits, closed op set, traverse bounds, tenant-scoped
//! dataset admissibility. No query text exists to inject into (structural).

use aelio_query::{check_admissible, parse_query, Dataset, DatasetRegistry, QOp};

fn q(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn limit_is_mandatory_and_positive() {
    assert!(parse_query(&q(r#"{"dataset":"clients","qop":"get","params":{},"limit":10}"#)).is_ok());
    // Missing limit ⇒ reject (§10.4 → G1).
    assert!(parse_query(&q(r#"{"dataset":"clients","qop":"get","params":{}}"#)).is_err());
    // Zero limit ⇒ reject.
    assert!(parse_query(&q(r#"{"dataset":"clients","qop":"get","limit":0}"#)).is_err());
}

#[test]
fn traverse_requires_max_depth_and_max_nodes() {
    let ok = parse_query(&q(r#"{"dataset":"graph","qop":"traverse","limit":50,"max_depth":3,"max_nodes":500}"#)).unwrap();
    assert_eq!(ok.qop, QOp::Traverse);
    assert_eq!(ok.max_depth, Some(3));
    // Missing max_nodes ⇒ reject.
    assert!(parse_query(&q(r#"{"dataset":"graph","qop":"traverse","limit":50,"max_depth":3}"#)).is_err());
}

#[test]
fn unknown_qop_is_rejected_no_text_surface() {
    // The closed op set is the entire query language — there is no "sql"/"text" op to inject into.
    assert!(parse_query(&q(r#"{"dataset":"x","qop":"raw_sql","limit":1}"#)).is_err());
}

#[test]
fn dataset_admissibility_is_tenant_scoped() {
    let mut reg = DatasetRegistry::default();
    reg.declare("tenant-a", Dataset { id: "clients".into(), modalities: vec![QOp::Get, QOp::TopkVector] });
    let query = parse_query(&q(r#"{"dataset":"clients","qop":"topk_vector","params":{},"limit":10}"#)).unwrap();

    // Declared for tenant-a with the right modality → admissible.
    assert!(check_admissible(&reg, "tenant-a", &query).is_ok());
    // Not declared for tenant-b → rejected (no fallback to global, §17.2).
    assert!(check_admissible(&reg, "tenant-b", &query).is_err());

    // Wrong modality for the dataset → rejected.
    let bm25 = parse_query(&q(r#"{"dataset":"clients","qop":"topk_bm25","limit":10}"#)).unwrap();
    assert!(check_admissible(&reg, "tenant-a", &bm25).is_err());
}
