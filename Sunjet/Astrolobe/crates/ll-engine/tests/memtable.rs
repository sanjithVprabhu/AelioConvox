//! Memtable MVCC visibility: versions, snapshots, tombstones.

use ll_engine::{Memtable, Op, Row, Value};

fn row(v: i64) -> Row {
    let mut r = Row::new();
    r.insert(1, Value::I64(v));
    r
}

#[test]
fn versions_are_visible_by_snapshot_lsn() {
    let mut m = Memtable::new();
    m.apply(42, Op::Put(row(10)), 5); // v1: xmin=5
    m.apply(42, Op::Put(row(20)), 8); // v2: xmin=8 (supersedes v1 at 8)

    // At snapshot 5..8 the old version is visible; at >=8 the new one.
    assert_eq!(m.get(42, 5).unwrap().get(&1), Some(&Value::I64(10)));
    assert_eq!(m.get(42, 7).unwrap().get(&1), Some(&Value::I64(10)));
    assert_eq!(m.get(42, 8).unwrap().get(&1), Some(&Value::I64(20)));
    assert_eq!(m.get(42, m.snapshot_lsn()).unwrap().get(&1), Some(&Value::I64(20)));

    // Before the row existed: invisible.
    assert!(m.get(42, 4).is_none());
}

#[test]
fn delete_creates_a_tombstone() {
    let mut m = Memtable::new();
    m.apply(1, Op::Put(row(100)), 2);
    m.apply(1, Op::Delete, 6);

    // Latest snapshot: gone. Earlier snapshot: still there.
    assert!(m.get(1, m.snapshot_lsn()).is_none());
    assert_eq!(m.get(1, 3).unwrap().get(&1), Some(&Value::I64(100)));
    assert_eq!(m.live_count(), 0);
}

#[test]
fn scan_returns_live_rows_in_order() {
    let mut m = Memtable::new();
    m.apply(3, Op::Put(row(3)), 1);
    m.apply(1, Op::Put(row(1)), 2);
    m.apply(2, Op::Put(row(2)), 3);
    m.apply(2, Op::Delete, 4);

    let live: Vec<u64> = m.live_rows().iter().map(|(id, _)| *id).collect();
    assert_eq!(live, vec![1, 3]); // sorted by row id, row 2 deleted
}
