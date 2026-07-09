//! Graph traversal must follow edges across segment boundaries: edges carry global ids, so
//! a path whose hops live in different `.vss` files is still reachable.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_query::{ColumnKind, Database, HybridQuery, Value};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TmpDir(PathBuf);
impl TmpDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("ll_xfg_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn schema() -> Vec<(&'static str, ColumnKind)> {
    vec![("name", ColumnKind::Utf8), ("cites", ColumnKind::Edge)]
}

fn node(name: &str, cites: Vec<u64>) -> Vec<(&'static str, Value)> {
    vec![
        ("name", Value::Utf8(name.to_string())),
        ("cites", Value::Edges(cites)),
    ]
}

#[test]
fn traversal_follows_edges_across_segments() {
    let dir = TmpDir::new();
    let mut db = Database::create(&dir.0).unwrap();
    db.create_table("papers", &schema()).unwrap();

    // Node 1 (→2) lives in segment 0; node 2 (→3) and node 3 live in segment 1; node 4 (→1)
    // stays in the memtable. The 1→2→3 path therefore spans two files, and 4→1→2→3 also
    // crosses the memtable/segment boundary.
    db.insert("papers", &node("one", vec![2])).unwrap(); // id 1
    db.flush().unwrap();
    db.insert("papers", &node("two", vec![3])).unwrap(); // id 2
    db.insert("papers", &node("three", vec![])).unwrap(); // id 3
    db.flush().unwrap();
    db.insert("papers", &node("four", vec![1])).unwrap(); // id 4 (memtable)

    // From node 1, depth 2 reaches 2 (seg0→seg1 edge) and 3 (within seg1).
    let r = db
        .query("papers", &HybridQuery::new(10).graph("cites", vec![1], 2))
        .unwrap();
    let ids: Vec<u64> = r.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&2), "1->2 crosses seg0->seg1: {ids:?}");
    assert!(ids.contains(&3), "1->2->3 spans two segments: {ids:?}");

    // From node 4 (memtable), depth 3 reaches 1, 2, 3 across memtable + both segments.
    let r2 = db
        .query("papers", &HybridQuery::new(10).graph("cites", vec![4], 3))
        .unwrap();
    let ids2: Vec<u64> = r2.iter().map(|(id, _)| *id).collect();
    assert!(
        ids2.contains(&1) && ids2.contains(&2) && ids2.contains(&3),
        "4->1->2->3 spans memtable + two segments: {ids2:?}"
    );
}
