//! The finale: one hybrid query, planned and executed across all four modalities and
//! across recent (memtable) + persisted (segment) data, fused with RRF.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_query::{ColumnKind, Database, HybridQuery, PredOp, Value};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TmpDir(PathBuf);
impl TmpDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("ll_db_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn doc(emb: [f32; 2], body: &str, follows: Vec<u64>, year: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("embedding", Value::Vector(emb.to_vec())),
        ("body", Value::Utf8(body.to_string())),
        ("cites", Value::Edges(follows)),
        ("year", Value::I64(year)),
    ]
}

fn schema() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("embedding", ColumnKind::Vector(2)),
        ("body", ColumnKind::Text),
        ("cites", ColumnKind::Edge),
        ("year", ColumnKind::I64),
    ]
}

#[test]
fn hybrid_query_spans_memtable_and_segments() {
    let dir = TmpDir::new();
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("documents", &schema()).unwrap();

    // Persisted rows (will be flushed to a segment).
    db.insert("documents", &doc([0.0, 0.0], "hedging derivatives volatility", vec![], 2020)).unwrap();
    db.insert("documents", &doc([10.0, 0.0], "cooking recipes", vec![], 2021)).unwrap();
    db.insert("documents", &doc([5.0, 0.0], "volatility models", vec![], 2022)).unwrap();
    db.flush().unwrap(); // rows 1,2,3 now live only in a segment

    // Recent rows (live only in the memtable).
    db.insert("documents", &doc([4.0, 0.0], "derivatives and volatility hedging", vec![], 2023)).unwrap();
    db.insert("documents", &doc([99.0, 0.0], "gardening", vec![], 2024)).unwrap();

    // Hybrid: near (4.5,0) AND mentions "volatility", fused with RRF, across both sources.
    let q = HybridQuery::new(3)
        .vector("embedding", vec![4.5, 0.0])
        .text("body", "volatility hedging");
    let res = db.query("documents", &q).unwrap();
    let ids: Vec<u64> = res.iter().map(|(id, _)| *id).collect();

    // The fused top-3 are exactly the on-topic rows — spanning the segment (1, 3) AND the
    // memtable (4) — while off-topic rows 2 (cooking) and 5 (gardening) are excluded.
    // This is one query plan ranking recent + persisted data across vector AND text.
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(sorted, vec![1, 3, 4], "fused top-3 = on-topic rows across both sources");
    assert!(ids.contains(&4), "a recent (memtable-only) row participates in the ranking");

    // The plan is a single hybrid operator tree over multiple sources.
    let explain = db.explain("documents", &q).unwrap();
    assert!(explain.contains("VectorSearch") && explain.contains("TextMatch") && explain.contains("RRF"));
}

#[test]
fn scalar_filter_and_table_isolation() {
    let dir = TmpDir::new();
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("documents", &schema()).unwrap();
    db.create_table("other", &[("embedding", ColumnKind::Vector(2))]).unwrap();

    db.insert("documents", &doc([1.0, 0.0], "alpha volatility", vec![], 2019)).unwrap();
    db.insert("documents", &doc([2.0, 0.0], "beta volatility", vec![], 2023)).unwrap();
    // A row in a different table with a colliding vector — must NOT appear in documents queries.
    db.insert("other", &[("embedding", Value::Vector(vec![1.0, 0.0]))]).unwrap();
    db.flush().unwrap();

    // "volatility" but only year >= 2020 → excludes the 2019 row.
    let q = HybridQuery::new(10)
        .text("body", "volatility")
        .filter("year", PredOp::Ge, Value::I64(2020));
    let ids: Vec<u64> = db.query("documents", &q).unwrap().iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![2], "scalar filter + table isolation");
}

#[test]
fn graph_constraint_in_hybrid_query() {
    let dir = TmpDir::new();
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("documents", &schema()).unwrap();

    // Citation chain: 1 -> 2 -> 3 ; 4 isolated. All mention "topic".
    db.insert("documents", &doc([0.0, 0.0], "topic one", vec![2], 2020)).unwrap();
    db.insert("documents", &doc([0.0, 0.0], "topic two", vec![3], 2020)).unwrap();
    db.insert("documents", &doc([0.0, 0.0], "topic three", vec![], 2020)).unwrap();
    db.insert("documents", &doc([0.0, 0.0], "topic four", vec![], 2020)).unwrap();
    db.flush().unwrap();

    // Text "topic", restricted to docs reachable from row 1 within 2 hops (→ {2,3}).
    let q = HybridQuery::new(10)
        .text("body", "topic")
        .graph("cites", vec![1], 2);
    let mut ids: Vec<u64> = db.query("documents", &q).unwrap().iter().map(|(id, _)| *id).collect();
    ids.sort();
    assert_eq!(ids, vec![2, 3], "only docs reachable from row 1 within 2 hops");
}
