//! Durable reopen: `Database::open` reconstructs the database from disk — flushed segments
//! plus the WAL tail past the checkpoint — without losing or double-counting committed data.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_query::{ColumnKind, Database, HybridQuery, Value};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TmpDir(PathBuf);
impl TmpDir {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("ll_rec_{}_{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn doc(emb: [f32; 2], body: &str, year: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("embedding", Value::Vector(emb.to_vec())),
        ("body", Value::Utf8(body.to_string())),
        ("year", Value::I64(year)),
    ]
}

fn schema() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("embedding", ColumnKind::Vector(2)),
        ("body", ColumnKind::Text),
        ("year", ColumnKind::I64),
    ]
}

#[test]
fn reopen_recovers_segment_rows_and_wal_tail_once() {
    let dir = TmpDir::new();
    {
        let mut db = Database::create(&dir.0).unwrap();
        db.create_table("docs", &schema()).unwrap();
        db.insert("docs", &doc([0.0, 0.0], "alpha volatility", 2020)).unwrap();
        db.insert("docs", &doc([10.0, 0.0], "beta", 2021)).unwrap();
        db.flush().unwrap(); // rows 1,2 durable in a segment; WAL truncated
        // Row 3 lives only in the WAL (no flush after it).
        db.insert("docs", &doc([5.0, 0.0], "gamma volatility", 2022)).unwrap();
    }

    let db = Database::open(&dir.0).unwrap();
    let res = db
        .query("docs", &HybridQuery::new(10).text("body", "volatility"))
        .unwrap();
    let ids: Vec<u64> = res.iter().map(|(id, _)| *id).collect();

    assert!(ids.contains(&1), "segment row recovered: {ids:?}");
    assert!(ids.contains(&3), "WAL-tail row recovered: {ids:?}");
    assert!(!ids.contains(&2), "row 2 has no 'volatility': {ids:?}");
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "no double-counting across WAL + segment: {ids:?}");
}

#[test]
fn reopen_continues_row_ids_and_flushes_again() {
    let dir = TmpDir::new();
    {
        let mut db = Database::create(&dir.0).unwrap();
        db.create_table("docs", &schema()).unwrap();
        assert_eq!(db.insert("docs", &doc([0.0, 0.0], "a", 1)).unwrap(), 1);
        assert_eq!(db.insert("docs", &doc([1.0, 0.0], "b volatility", 2)).unwrap(), 2);
        db.flush().unwrap();
    }

    // Reopen: the next id must continue past the persisted rows, not collide.
    let mut db = Database::open(&dir.0).unwrap();
    let id3 = db.insert("docs", &doc([2.0, 0.0], "c volatility", 3)).unwrap();
    assert_eq!(id3, 3, "row id continues after reopen");
    db.flush().unwrap(); // a second segment

    // Reopen once more: both segments load and queries span them.
    let db = Database::open(&dir.0).unwrap();
    let res = db
        .query("docs", &HybridQuery::new(10).text("body", "volatility"))
        .unwrap();
    let ids: Vec<u64> = res.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&2) && ids.contains(&3), "rows from both segments: {ids:?}");
}
