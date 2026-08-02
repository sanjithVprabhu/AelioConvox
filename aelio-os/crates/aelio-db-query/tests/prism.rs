//! Prism envelope → HybridQuery → projected select.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aelio_db_query::{
    execute_prism, lower_prism, ColumnKind, Database, HashEmbedder, PrismEmbedder, Value,
};
use aelio_query::{
    parse_prism, recall, CollectionSchema, PrismColKind, PrismColumn, RecallModality,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TmpDir(PathBuf);
impl TmpDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("aelio_prism_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn prompts_table() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("id", ColumnKind::Utf8),
        ("body", ColumnKind::Text),
        ("embedding", ColumnKind::Vector(4)),
        ("category", ColumnKind::Utf8),
        ("active", ColumnKind::Bool),
        ("version", ColumnKind::I64),
    ]
}

fn prism_schema() -> CollectionSchema {
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
                dim: 4,
                model: "hash-4".into(),
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
    CollectionSchema {
        name: "prompts".into(),
        columns,
        default_select: vec!["id".into(), "body".into(), "version".into()],
        default_limit: 5,
    }
}

#[test]
fn prism_text_recall_projects_select_only() {
    let dir = TmpDir::new();
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &prompts_table()).unwrap();
    let emb = HashEmbedder { dim: 4 };
    let v1 = emb
        .embed("hash-4", "how to reply to a customer message")
        .unwrap();
    let v2 = emb.embed("hash-4", "cooking pasta recipes").unwrap();
    db.insert(
        "prompts",
        &[
            ("id", Value::Utf8("p1".into())),
            ("body", Value::Utf8("template for customer reply".into())),
            ("embedding", Value::Vector(v1)),
            ("category", Value::Utf8("reply".into())),
            ("active", Value::Bool(true)),
            ("version", Value::I64(1)),
        ],
    )
    .unwrap();
    db.insert(
        "prompts",
        &[
            ("id", Value::Utf8("p2".into())),
            ("body", Value::Utf8("pasta boiling water salt".into())),
            ("embedding", Value::Vector(v2)),
            ("category", Value::Utf8("food".into())),
            ("active", Value::Bool(true)),
            ("version", Value::I64(1)),
        ],
    )
    .unwrap();

    let schema = prism_schema();
    let q = recall(&schema, "customer reply", RecallModality::Fulltext).unwrap();
    let models: BTreeMap<String, String> = [("embedding".into(), "hash-4".into())]
        .into_iter()
        .collect();
    let hits = execute_prism(&db, &q, &models, Some(&emb)).unwrap();
    assert!(!hits.is_empty());
    let top = &hits[0];
    assert!(top.fields.contains_key("id"));
    assert!(top.fields.contains_key("body"));
    assert!(top.fields.contains_key("version"));
    assert!(!top.fields.contains_key("embedding"));
    assert!(!top.fields.contains_key("category"));
}

#[test]
fn prism_envelope_with_where_and_vector_embed() {
    let dir = TmpDir::new();
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &prompts_table()).unwrap();
    let emb = HashEmbedder { dim: 4 };
    let need = "how to reply to a customer message";
    let v = emb.embed("hash-4", need).unwrap();
    db.insert(
        "prompts",
        &[
            ("id", Value::Utf8("p1".into())),
            ("body", Value::Utf8("reply guidance".into())),
            ("embedding", Value::Vector(v)),
            ("category", Value::Utf8("reply".into())),
            ("active", Value::Bool(true)),
            ("version", Value::I64(3)),
        ],
    )
    .unwrap();

    let j = serde_json::json!({
        "from": "prompts",
        "match": [
            {"kind": "vector", "on": "embedding", "embed": need},
            {"kind": "fusion", "method": "rrf", "rrf_k": 60.0}
        ],
        "where": [
            {"col": "category", "op": "eq", "value": "reply"},
            {"col": "active", "op": "eq", "value": true}
        ],
        "select": ["id", "body", "version"],
        "limit": 10,
        "into": "candidates"
    });
    let q = parse_prism(&j).unwrap();
    let models: BTreeMap<String, String> = [("embedding".into(), "hash-4".into())]
        .into_iter()
        .collect();
    let hybrid = lower_prism(&q, &models, Some(&emb)).unwrap();
    assert!(hybrid.vector.is_some());
    assert_eq!(hybrid.filters.len(), 2);
    assert!((hybrid.rrf_k - 60.0).abs() < f32::EPSILON);

    let hits = execute_prism(&db, &q, &models, Some(&emb)).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].fields.get("id"), Some(&Value::Utf8("p1".into())));
}
