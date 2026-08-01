//! Adversarial Prism suite — close loopholes around schema, dump, modality, edit, delete, limit.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aelio_db_query::{
    execute_prism, lower_prism, ColumnKind, Database, HashEmbedder, PrismEmbedder, Value,
};
use aelio_query::{
    check_prism, parse_prism, prism_to_json, recall, CollectionRegistry, CollectionSchema,
    PrismColKind, PrismColumn, RecallModality, MAX_QUERY_LIMIT,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p =
            std::env::temp_dir().join(format!("aelio_prism_adv_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn table() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("id", ColumnKind::Utf8),
        ("body", ColumnKind::Text),
        ("embedding", ColumnKind::Vector(8)),
        ("category", ColumnKind::Utf8),
        ("active", ColumnKind::Bool),
        ("version", ColumnKind::I64),
        ("links", ColumnKind::Edge),
    ]
}

fn schema() -> CollectionSchema {
    let mut columns = BTreeMap::new();
    columns.insert(
        "id".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    columns.insert(
        "body".into(),
        PrismColumn {
            kind: PrismColKind::Text,
        },
    );
    columns.insert(
        "embedding".into(),
        PrismColumn {
            kind: PrismColKind::Vector {
                dim: 8,
                model: "hash-8".into(),
            },
        },
    );
    columns.insert(
        "category".into(),
        PrismColumn {
            kind: PrismColKind::Utf8,
        },
    );
    columns.insert(
        "active".into(),
        PrismColumn {
            kind: PrismColKind::Bool,
        },
    );
    columns.insert(
        "version".into(),
        PrismColumn {
            kind: PrismColKind::Int,
        },
    );
    columns.insert(
        "links".into(),
        PrismColumn {
            kind: PrismColKind::Edge,
        },
    );
    CollectionSchema {
        name: "prompts".into(),
        columns,
        default_select: vec!["id".into(), "body".into(), "version".into()],
        default_limit: 10,
    }
}

fn models() -> BTreeMap<String, String> {
    [("embedding".into(), "hash-8".into())]
        .into_iter()
        .collect()
}

fn seed(db: &mut Database, emb: &HashEmbedder, n: usize) {
    for i in 0..n {
        let body = if i % 3 == 0 {
            format!("customer reply template number {i}")
        } else if i % 3 == 1 {
            format!("otp phone verification step {i}")
        } else {
            format!("unrelated cooking recipe {i}")
        };
        let category = if i % 3 == 0 {
            "reply"
        } else if i % 3 == 1 {
            "login"
        } else {
            "food"
        };
        let vector = emb.embed("hash-8", &body).unwrap();
        db.insert(
            "prompts",
            &[
                ("id", Value::Utf8(format!("p{i}"))),
                ("body", Value::Utf8(body)),
                ("embedding", Value::Vector(vector)),
                ("category", Value::Utf8(category.into())),
                ("active", Value::Bool(true)),
                ("version", Value::I64(1)),
                ("links", Value::Edges(vec![])),
            ],
        )
        .unwrap();
    }
}

#[test]
fn rejects_unknown_field_closed_envelope() {
    let j = serde_json::json!({
        "from": "prompts",
        "where": [{"col": "active", "op": "eq", "value": true}],
        "select": ["id"],
        "limit": 1,
        "join": "evil"
    });
    assert!(parse_prism(&j).unwrap_err().contains("unknown Prism field"));
}

#[test]
fn rejects_limit_zero_and_over_cap() {
    let base = |limit: u64| {
        serde_json::json!({
            "from": "prompts",
            "where": [{"col": "active", "op": "eq", "value": true}],
            "select": ["id"],
            "limit": limit
        })
    };
    assert!(parse_prism(&base(0)).is_err());
    assert!(parse_prism(&base(MAX_QUERY_LIMIT + 1)).is_err());
    assert!(parse_prism(&base(MAX_QUERY_LIMIT)).is_ok());
}

#[test]
fn rejects_empty_text_and_nan_vector() {
    assert!(parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "   "},
        "select": ["id"],
        "limit": 1
    }))
    .is_err());
    assert!(parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "vector", "on": "embedding", "vector": [1.0, f64::NAN]},
        "select": ["id"],
        "limit": 1
    }))
    .is_err());
}

#[test]
fn check_rejects_modality_column_mismatch() {
    let mut reg = CollectionRegistry::default();
    reg.declare("demo", schema());
    let text_on_vector = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "embedding", "query": "x"},
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    assert!(check_prism(&reg, "demo", &text_on_vector)
        .unwrap_err()
        .contains("Text column"));

    let vector_on_text = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "vector", "on": "body", "embed": "x"},
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    assert!(check_prism(&reg, "demo", &vector_on_text)
        .unwrap_err()
        .contains("Vector column"));

    let bad_dim = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "vector", "on": "embedding", "vector": [0.1, 0.2]},
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    assert!(check_prism(&reg, "demo", &bad_dim)
        .unwrap_err()
        .contains("dim"));
}

#[test]
fn rejects_two_text_matches_and_missing_embedder() {
    let err = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": [
            {"kind": "text", "on": "body", "query": "a"},
            {"kind": "text", "on": "body", "query": "b"}
        ],
        "select": ["id"],
        "limit": 1
    }))
    .unwrap_err();
    assert!(
        err.contains("at most one text") || err.contains("fusion"),
        "{err}"
    );

    let embed_q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "vector", "on": "embedding", "embed": "hello"},
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    let err = lower_prism(&embed_q, &models(), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("embedder"), "{err}");
}

#[test]
fn rejects_hybrid_without_explicit_fusion() {
    let err = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": [
            {"kind": "text", "on": "body", "query": "a"},
            {"kind": "vector", "on": "embedding", "embed": "a"}
        ],
        "select": ["id"],
        "limit": 1
    }))
    .unwrap_err();
    assert!(err.contains("fusion"), "{err}");
}

#[test]
fn rejects_where_map_and_list_values() {
    assert!(parse_prism(&serde_json::json!({
        "from": "prompts",
        "where": [{"col": "category", "op": "eq", "value": {"evil": 1}}],
        "select": ["id"],
        "limit": 1
    }))
    .unwrap_err()
    .contains("map"));
    assert!(parse_prism(&serde_json::json!({
        "from": "prompts",
        "where": [{"col": "category", "op": "eq", "value": [1, 2]}],
        "select": ["id"],
        "limit": 1
    }))
    .unwrap_err()
    .contains("list"));
}

#[test]
fn graph_reachability_and_budget_surface() {
    let dir = TmpDir::new("graph");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table(
        "prompts",
        &[
            ("id", ColumnKind::Utf8),
            ("body", ColumnKind::Text),
            ("embedding", ColumnKind::Vector(8)),
            ("category", ColumnKind::Utf8),
            ("active", ColumnKind::Bool),
            ("version", ColumnKind::I64),
            ("links", ColumnKind::Edge),
        ],
    )
    .unwrap();
    let emb = HashEmbedder { dim: 8 };
    let a = db
        .insert(
            "prompts",
            &[
                ("id", Value::Utf8("a".into())),
                ("body", Value::Utf8("seed node".into())),
                (
                    "embedding",
                    Value::Vector(emb.embed("hash-8", "seed node").unwrap()),
                ),
                ("category", Value::Utf8("x".into())),
                ("active", Value::Bool(true)),
                ("version", Value::I64(1)),
                ("links", Value::Edges(vec![])), // filled after we know ids
            ],
        )
        .unwrap();
    let b = db
        .insert(
            "prompts",
            &[
                ("id", Value::Utf8("b".into())),
                ("body", Value::Utf8("neighbor node".into())),
                (
                    "embedding",
                    Value::Vector(emb.embed("hash-8", "neighbor node").unwrap()),
                ),
                ("category", Value::Utf8("x".into())),
                ("active", Value::Bool(true)),
                ("version", Value::I64(1)),
                ("links", Value::Edges(vec![])),
            ],
        )
        .unwrap();
    db.update("prompts", a, &[("links", Value::Edges(vec![b]))])
        .unwrap();

    let q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {
            "kind": "graph",
            "on": "links",
            "seeds": [a],
            "max_depth": 1,
            "max_nodes": 10
        },
        "select": ["id"],
        "limit": 10
    }))
    .unwrap();
    let mut reg = CollectionRegistry::default();
    reg.declare("demo", schema());
    check_prism(&reg, "demo", &q).unwrap();
    let hits = execute_prism(&db, &q, &models(), Some(&emb)).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fields.get("id"), Some(&Value::Utf8("b".into())));

    // Tiny max_nodes must surface as Budget error, not empty success.
    let tight = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {
            "kind": "graph",
            "on": "links",
            "seeds": [a],
            "max_depth": 1,
            "max_nodes": 1
        },
        "select": ["id"],
        "limit": 10
    }))
    .unwrap();
    // seeds.len()==1 and max_nodes==1 is allowed by validate; expansion of neighbor trips visited.
    let err = execute_prism(&db, &tight, &models(), Some(&emb))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("budget") || err.contains("Visited") || err.contains("Frontier"),
        "{err}"
    );
}

#[test]
fn tenant_isolation_on_collection_registry() {
    let mut reg = CollectionRegistry::default();
    reg.declare("tenant-a", schema());
    let q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "where": [{"col": "active", "op": "eq", "value": true}],
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    assert!(check_prism(&reg, "tenant-a", &q).is_ok());
    assert!(check_prism(&reg, "tenant-b", &q)
        .unwrap_err()
        .contains("not declared"));
}

#[test]
fn unknown_table_is_hard_error() {
    let dir = TmpDir::new("notable");
    let db = Database::create(&dir.0).unwrap();
    let q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "where": [{"col": "active", "op": "eq", "value": true}],
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    let err = execute_prism(&db, &q, &models(), None)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("does not exist") || err.contains("no such table") || err.contains("prism io"),
        "{err}"
    );
}

#[test]
fn limit_is_hard_cap_under_load() {
    let dir = TmpDir::new("limit");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    let emb = HashEmbedder { dim: 8 };
    seed(&mut db, &emb, 40);

    let q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "customer reply"},
        "select": ["id", "body"],
        "limit": 3
    }))
    .unwrap();
    let hits = execute_prism(&db, &q, &models(), Some(&emb)).unwrap();
    assert!(
        hits.len() <= 3,
        "limit must cap results, got {}",
        hits.len()
    );
    assert!(!hits.is_empty());
}

#[test]
fn projection_never_leaks_unselected_fields() {
    let dir = TmpDir::new("proj");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    let emb = HashEmbedder { dim: 8 };
    seed(&mut db, &emb, 5);

    let q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "reply"},
        "select": ["id"],
        "limit": 5
    }))
    .unwrap();
    let hits = execute_prism(&db, &q, &models(), Some(&emb)).unwrap();
    assert!(!hits.is_empty());
    for h in &hits {
        assert_eq!(h.fields.len(), 1);
        assert!(h.fields.contains_key("id"));
        assert!(!h.fields.contains_key("body"));
        assert!(!h.fields.contains_key("embedding"));
        assert!(!h.fields.contains_key("category"));
    }
}

#[test]
fn where_and_filters_exclude_wrong_category() {
    let dir = TmpDir::new("where");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    let emb = HashEmbedder { dim: 8 };
    seed(&mut db, &emb, 12);

    let q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "template"},
        "where": [
            {"col": "category", "op": "eq", "value": "reply"},
            {"col": "active", "op": "eq", "value": true}
        ],
        "select": ["id", "category"],
        "limit": 20
    }))
    .unwrap();
    let hits = execute_prism(&db, &q, &models(), Some(&emb)).unwrap();
    assert!(!hits.is_empty());
    for h in &hits {
        assert_eq!(h.fields.get("category"), Some(&Value::Utf8("reply".into())));
    }
}

#[test]
fn edit_reembed_changes_vector_ranking() {
    let dir = TmpDir::new("reembed");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    let emb = HashEmbedder { dim: 8 };

    let a = db
        .insert(
            "prompts",
            &[
                ("id", Value::Utf8("a".into())),
                ("body", Value::Utf8("alpha alpha alpha".into())),
                (
                    "embedding",
                    Value::Vector(emb.embed("hash-8", "alpha alpha alpha").unwrap()),
                ),
                ("category", Value::Utf8("x".into())),
                ("active", Value::Bool(true)),
                ("version", Value::I64(1)),
                ("links", Value::Edges(vec![])),
            ],
        )
        .unwrap();
    db.insert(
        "prompts",
        &[
            ("id", Value::Utf8("b".into())),
            ("body", Value::Utf8("beta beta beta".into())),
            (
                "embedding",
                Value::Vector(emb.embed("hash-8", "beta beta beta").unwrap()),
            ),
            ("category", Value::Utf8("x".into())),
            ("active", Value::Bool(true)),
            ("version", Value::I64(1)),
            ("links", Value::Edges(vec![])),
        ],
    )
    .unwrap();

    let q = recall(&schema(), "alpha alpha alpha", RecallModality::Vector).unwrap();
    let hits = execute_prism(&db, &q, &models(), Some(&emb)).unwrap();
    assert_eq!(hits[0].fields.get("id"), Some(&Value::Utf8("a".into())));

    // Rewrite a toward beta semantics.
    let new_body = "beta beta beta beta";
    assert!(db
        .update(
            "prompts",
            a,
            &[
                ("body", Value::Utf8(new_body.into())),
                (
                    "embedding",
                    Value::Vector(emb.embed("hash-8", new_body).unwrap()),
                ),
                ("version", Value::I64(2)),
            ],
        )
        .unwrap());

    let q = recall(&schema(), "beta beta beta", RecallModality::Vector).unwrap();
    let hits = execute_prism(&db, &q, &models(), Some(&emb)).unwrap();
    // Both a and b are beta-like; top should still be finite and include updated a as live.
    assert!(!hits.is_empty());
    let ids: Vec<_> = hits
        .iter()
        .filter_map(|h| match h.fields.get("id") {
            Some(Value::Utf8(s)) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(
        ids.contains(&"a".to_string()),
        "updated row must remain live: {ids:?}"
    );
}

#[test]
fn delete_removes_from_text_and_vector_recall() {
    let dir = TmpDir::new("del");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    let emb = HashEmbedder { dim: 8 };
    let body = "unique zebra token for deletion recall";
    let id = db
        .insert(
            "prompts",
            &[
                ("id", Value::Utf8("zebra".into())),
                ("body", Value::Utf8(body.into())),
                (
                    "embedding",
                    Value::Vector(emb.embed("hash-8", body).unwrap()),
                ),
                ("category", Value::Utf8("x".into())),
                ("active", Value::Bool(true)),
                ("version", Value::I64(1)),
                ("links", Value::Edges(vec![])),
            ],
        )
        .unwrap();

    let text_q = recall(&schema(), "zebra token", RecallModality::Fulltext).unwrap();
    assert!(!execute_prism(&db, &text_q, &models(), Some(&emb))
        .unwrap()
        .is_empty());

    assert!(db.delete("prompts", id).unwrap());

    assert!(
        execute_prism(&db, &text_q, &models(), Some(&emb))
            .unwrap()
            .is_empty(),
        "deleted row must vanish from text recall"
    );
    let vec_q = recall(&schema(), body, RecallModality::Vector).unwrap();
    assert!(
        execute_prism(&db, &vec_q, &models(), Some(&emb))
            .unwrap()
            .iter()
            .all(|h| h.fields.get("id") != Some(&Value::Utf8("zebra".into()))),
        "deleted row must vanish from vector recall"
    );
}

#[test]
fn hybrid_fusion_and_json_round_trip() {
    let dir = TmpDir::new("hybrid");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    let emb = HashEmbedder { dim: 8 };
    seed(&mut db, &emb, 15);

    let q = recall(&schema(), "customer reply template", RecallModality::Hybrid).unwrap();
    let json = prism_to_json(&q);
    let q2 = parse_prism(&json).unwrap();
    assert_eq!(q.from, q2.from);
    assert_eq!(q.select, q2.select);
    assert_eq!(q.limit, q2.limit);
    assert_eq!(q.matches.len(), q2.matches.len());

    let hits = execute_prism(&db, &q2, &models(), Some(&emb)).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.len() <= q.limit as usize);
}

#[test]
fn filter_only_with_where_key_match_works() {
    let dir = TmpDir::new("key");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    let emb = HashEmbedder { dim: 8 };
    seed(&mut db, &emb, 6);

    let q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "key", "on": "id", "value": "p0"},
        "select": ["id", "body", "version"],
        "limit": 5
    }))
    .unwrap();
    let mut reg = CollectionRegistry::default();
    reg.declare("demo", schema());
    check_prism(&reg, "demo", &q).unwrap();
    let hits = execute_prism(&db, &q, &models(), Some(&emb)).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fields.get("id"), Some(&Value::Utf8("p0".into())));
}

#[test]
fn flush_compact_path_still_prism_safe() {
    let dir = TmpDir::new("flush");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    let emb = HashEmbedder { dim: 8 };
    seed(&mut db, &emb, 10);
    db.flush().unwrap();
    seed(&mut db, &emb, 5); // more in memtable; ids will collide on id string p0.. — still ok for recall

    let q = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "otp phone"},
        "where": [{"col": "category", "op": "eq", "value": "login"}],
        "select": ["id", "category"],
        "limit": 10
    }))
    .unwrap();
    let hits = execute_prism(&db, &q, &models(), Some(&emb)).unwrap();
    assert!(!hits.is_empty());
    for h in hits {
        assert_eq!(h.fields.get("category"), Some(&Value::Utf8("login".into())));
    }
}

#[test]
fn nested_clauses_are_closed_and_vector_input_is_unambiguous() {
    let extra = serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "x", "boost": 1000},
        "select": ["id"],
        "limit": 1
    });
    assert!(parse_prism(&extra).unwrap_err().contains("closed clause"));

    let both = serde_json::json!({
        "from": "prompts",
        "match": {
            "kind": "vector", "on": "embedding", "embed": "x", "vector": vec![0.0; 8]
        },
        "select": ["id"],
        "limit": 1
    });
    assert!(parse_prism(&both).unwrap_err().contains("exactly one"));

    let extra_where = serde_json::json!({
        "from": "prompts",
        "where": {"col": "active", "op": "eq", "value": true, "or": true},
        "select": ["id"],
        "limit": 1
    });
    assert!(parse_prism(&extra_where)
        .unwrap_err()
        .contains("closed clause"));
}

#[test]
fn into_and_graph_resource_bounds_are_mandatory() {
    let invalid_into = serde_json::json!({
        "from": "prompts",
        "where": {"col": "active", "op": "eq", "value": true},
        "select": ["id"],
        "limit": 1,
        "into": "[0].computed"
    });
    assert!(parse_prism(&invalid_into).unwrap_err().contains("into"));

    let missing_nodes = serde_json::json!({
        "from": "prompts",
        "match": {"kind": "graph", "on": "links", "seeds": [1], "max_depth": 1},
        "select": ["id"],
        "limit": 1
    });
    assert!(parse_prism(&missing_nodes)
        .unwrap_err()
        .contains("max_nodes"));
}

#[test]
fn programmatic_query_cannot_bypass_parser_limit() {
    let mut query = parse_prism(&serde_json::json!({
        "from": "prompts",
        "where": {"col": "active", "op": "eq", "value": true},
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    query.limit = MAX_QUERY_LIMIT + 1;
    let error = lower_prism(&query, &models(), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("limit"), "{error}");
}

#[test]
fn execution_fails_closed_on_projection_types_and_embed_dimensions() {
    let dir = TmpDir::new("physical-schema");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();

    let unknown_projection = parse_prism(&serde_json::json!({
        "from": "prompts",
        "where": {"col": "active", "op": "eq", "value": true},
        "select": ["secret_typo"],
        "limit": 1
    }))
    .unwrap();
    let error = execute_prism(&db, &unknown_projection, &models(), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("select column"), "{error}");

    let wrong_type = parse_prism(&serde_json::json!({
        "from": "prompts",
        "where": {"col": "active", "op": "eq", "value": "true"},
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    let error = execute_prism(&db, &wrong_type, &models(), None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("incompatible"), "{error}");

    let vector = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "vector", "on": "embedding", "embed": "hello"},
        "select": ["id"],
        "limit": 1
    }))
    .unwrap();
    let wrong_dim = HashEmbedder { dim: 7 };
    let error = execute_prism(&db, &vector, &models(), Some(&wrong_dim))
        .unwrap_err()
        .to_string();
    assert!(error.contains("dimension"), "{error}");
}

#[test]
fn storage_rejects_values_that_would_poison_prism_indexes() {
    let dir = TmpDir::new("invalid-storage");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table()).unwrap();
    assert!(db
        .insert("prompts", &[("embedding", Value::Vector(vec![0.0; 7]))])
        .unwrap_err()
        .to_string()
        .contains("dimension"));
    assert!(db
        .insert("prompts", &[("active", Value::Utf8("true".into()))])
        .unwrap_err()
        .to_string()
        .contains("does not match schema"));
    assert!(db
        .insert(
            "prompts",
            &[("version", Value::I64(1)), ("version", Value::I64(2))]
        )
        .unwrap_err()
        .to_string()
        .contains("duplicate column"));
}
