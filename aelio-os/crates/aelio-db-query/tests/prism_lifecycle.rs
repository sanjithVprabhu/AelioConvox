//! End-to-end Prism collection lifecycle: store → recall → edit → recall → delete.
//!
//! This is the “does Prism work for real collection CRUD?” harness.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aelio_db_query::{
    execute_prism, ColumnKind, Database, HashEmbedder, PrismEmbedder, Value,
};
use aelio_query::{
    check_prism, parse_prism, recall, CollectionRegistry, CollectionSchema, PrismColKind,
    PrismColumn, RecallModality,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "aelio_prism_life_{tag}_{}_{n}",
            std::process::id()
        ));
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

fn table_schema() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("id", ColumnKind::Utf8),
        ("body", ColumnKind::Text),
        ("embedding", ColumnKind::Vector(8)),
        ("category", ColumnKind::Utf8),
        ("active", ColumnKind::Bool),
        ("version", ColumnKind::I64),
    ]
}

fn prism_collection() -> CollectionSchema {
    let mut columns = BTreeMap::new();
    columns.insert("id".into(), PrismColumn { kind: PrismColKind::Utf8 });
    columns.insert("body".into(), PrismColumn { kind: PrismColKind::Text });
    columns.insert(
        "embedding".into(),
        PrismColumn {
            kind: PrismColKind::Vector {
                dim: 8,
                model: "hash-8".into(),
            },
        },
    );
    columns.insert("category".into(), PrismColumn { kind: PrismColKind::Utf8 });
    columns.insert("active".into(), PrismColumn { kind: PrismColKind::Bool });
    columns.insert("version".into(), PrismColumn { kind: PrismColKind::Int });
    CollectionSchema {
        name: "prompts".into(),
        columns,
        default_select: vec!["id".into(), "body".into(), "version".into()],
        default_limit: 10,
    }
}

fn models() -> BTreeMap<String, String> {
    [("embedding".into(), "hash-8".into())].into_iter().collect()
}

fn row(
    emb: &HashEmbedder,
    id: &str,
    body: &str,
    category: &str,
    active: bool,
    version: i64,
) -> Vec<(&'static str, Value)> {
    let vector = emb.embed("hash-8", body).expect("embed");
    vec![
        ("id", Value::Utf8(id.into())),
        ("body", Value::Utf8(body.into())),
        ("embedding", Value::Vector(vector)),
        ("category", Value::Utf8(category.into())),
        ("active", Value::Bool(active)),
        ("version", Value::I64(version)),
    ]
}

#[test]
fn collection_store_retrieve_edit_delete_via_prism() {
    let dir = TmpDir::new("crud");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table_schema()).unwrap();

    let emb = HashEmbedder { dim: 8 };
    let model_map = models();
    let coll = prism_collection();
    let mut reg = CollectionRegistry::default();
    reg.declare("demo", coll.clone());

    // ── STORE three prompts ──────────────────────────────────────────────
    let r1 = db
        .insert(
            "prompts",
            &row(
                &emb,
                "reply.v1",
                "how to reply to a customer message politely",
                "reply",
                true,
                1,
            ),
        )
        .unwrap();
    let r2 = db
        .insert(
            "prompts",
            &row(
                &emb,
                "otp.v1",
                "ask the user for a phone number then wait",
                "login",
                true,
                1,
            ),
        )
        .unwrap();
    let _r3 = db
        .insert(
            "prompts",
            &row(
                &emb,
                "food.v1",
                "pasta boiling water salt olive oil",
                "food",
                true,
                1,
            ),
        )
        .unwrap();

    // ── RETRIEVE fulltext ────────────────────────────────────────────────
    let q_text = recall(
        &coll,
        "reply to a customer message",
        RecallModality::Fulltext,
    )
    .unwrap();
    check_prism(&reg, "demo", &q_text).unwrap();
    let hits = execute_prism(&db, &q_text, &model_map, Some(&emb)).unwrap();
    assert!(
        !hits.is_empty(),
        "fulltext recall must find reply prompt: {hits:?}"
    );
    assert!(
        hits.iter().any(|h| h.fields.get("id") == Some(&Value::Utf8("reply.v1".into()))),
        "expected reply.v1 in fulltext hits: {hits:?}"
    );
    // Projection: no embedding leaked
    assert!(hits.iter().all(|h| !h.fields.contains_key("embedding")));

    // ── RETRIEVE vector ──────────────────────────────────────────────────
    let q_vec = recall(
        &coll,
        "how to reply to a customer message politely",
        RecallModality::Vector,
    )
    .unwrap();
    let hits = execute_prism(&db, &q_vec, &model_map, Some(&emb)).unwrap();
    assert!(
        !hits.is_empty(),
        "vector recall must return hits: {hits:?}"
    );
    assert_eq!(
        hits[0].fields.get("id"),
        Some(&Value::Utf8("reply.v1".into())),
        "nearest neighbor should be reply.v1, got {hits:?}"
    );

    // ── RETRIEVE with where filter (envelope) ────────────────────────────
    let filtered = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "phone number"},
        "where": [
            {"col": "category", "op": "eq", "value": "login"},
            {"col": "active", "op": "eq", "value": true}
        ],
        "select": ["id", "body", "version"],
        "limit": 5
    }))
    .unwrap();
    check_prism(&reg, "demo", &filtered).unwrap();
    let hits = execute_prism(&db, &filtered, &model_map, Some(&emb)).unwrap();
    assert_eq!(hits.len(), 1, "login filter should isolate otp.v1: {hits:?}");
    assert_eq!(
        hits[0].fields.get("id"),
        Some(&Value::Utf8("otp.v1".into()))
    );
    assert_eq!(hits[0].row_id, r2);

    // ── EDIT (update body + re-embed + bump version) ─────────────────────
    let new_body = "ask the user for a phone number for OTP login flow";
    let new_vec = emb.embed("hash-8", new_body).unwrap();
    assert!(
        db.update(
            "prompts",
            r2,
            &[
                ("body", Value::Utf8(new_body.into())),
                ("embedding", Value::Vector(new_vec)),
                ("version", Value::I64(2)),
            ],
        )
        .unwrap(),
        "update must return true for live row"
    );

    let after_edit = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "OTP login"},
        "where": [{"col": "id", "op": "eq", "value": "otp.v1"}],
        "select": ["id", "body", "version"],
        "limit": 3
    }))
    .unwrap();
    let hits = execute_prism(&db, &after_edit, &model_map, Some(&emb)).unwrap();
    assert_eq!(hits.len(), 1, "edited row must be recallable: {hits:?}");
    assert_eq!(
        hits[0].fields.get("version"),
        Some(&Value::I64(2)),
        "version must bump after edit"
    );
    assert_eq!(
        hits[0].fields.get("body"),
        Some(&Value::Utf8(new_body.into()))
    );

    // ── soft deactivate via edit + filtered recall must miss ─────────────
    assert!(db
        .update("prompts", r1, &[("active", Value::Bool(false))])
        .unwrap());
    let active_only = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "customer message"},
        "where": [{"col": "active", "op": "eq", "value": true}],
        "select": ["id", "body", "version"],
        "limit": 10
    }))
    .unwrap();
    let hits = execute_prism(&db, &active_only, &model_map, Some(&emb)).unwrap();
    assert!(
        hits.iter()
            .all(|h| h.fields.get("id") != Some(&Value::Utf8("reply.v1".into()))),
        "deactivated reply.v1 must not appear in active=true recall: {hits:?}"
    );

    // ── DELETE ───────────────────────────────────────────────────────────
    assert!(db.delete("prompts", r2).unwrap());
    let gone = parse_prism(&serde_json::json!({
        "from": "prompts",
        "match": {"kind": "text", "on": "body", "query": "OTP login"},
        "where": [{"col": "id", "op": "eq", "value": "otp.v1"}],
        "select": ["id", "body", "version"],
        "limit": 5
    }))
    .unwrap();
    let hits = execute_prism(&db, &gone, &model_map, Some(&emb)).unwrap();
    assert!(
        hits.is_empty(),
        "deleted otp.v1 must not recall: {hits:?}"
    );
}

#[test]
fn collection_survives_flush_then_prism_recall() {
    let dir = TmpDir::new("flush");
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("prompts", &table_schema()).unwrap();
    let emb = HashEmbedder { dim: 8 };
    let model_map = models();
    let coll = prism_collection();

    db.insert(
        "prompts",
        &row(
            &emb,
            "persist.v1",
            "durable customer reply template after flush",
            "reply",
            true,
            1,
        ),
    )
    .unwrap();
    db.flush().unwrap();

    // New insert only in memtable — both must be recallable together.
    db.insert(
        "prompts",
        &row(
            &emb,
            "recent.v1",
            "recent customer reply guidance still in memtable",
            "reply",
            true,
            1,
        ),
    )
    .unwrap();

    let q = recall(&coll, "customer reply", RecallModality::Fulltext).unwrap();
    let hits = execute_prism(&db, &q, &model_map, Some(&emb)).unwrap();
    let ids: Vec<_> = hits
        .iter()
        .filter_map(|h| match h.fields.get("id") {
            Some(Value::Utf8(s)) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        ids.contains(&"persist.v1") && ids.contains(&"recent.v1"),
        "flush must not hide either row from Prism: {ids:?}"
    );
}
