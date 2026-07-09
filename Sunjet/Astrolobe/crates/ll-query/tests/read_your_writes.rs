//! Recent, un-flushed memtable rows are queryable through every modality (vector, text,
//! graph) with correct MVCC — the read-your-writes guarantee across access patterns.

use ll_engine::{Memtable, Op, Row, Value};
use ll_query::Source;

const VEC: u32 = 1;
const BODY: u32 = 2;
const FOLLOWS: u32 = 3;

fn put(m: &mut Memtable, id: u64, lsn: u64, build: impl FnOnce(&mut Row)) {
    let mut r = Row::new();
    build(&mut r);
    m.apply(id, Op::Put(r), lsn);
}

#[test]
fn vector_search_sees_recent_writes_and_mvcc() {
    let mut m = Memtable::new();
    put(&mut m, 1, 1, |r| { r.insert(VEC, Value::Vector(vec![0.0, 0.0])); });
    put(&mut m, 2, 2, |r| { r.insert(VEC, Value::Vector(vec![10.0, 0.0])); });
    put(&mut m, 3, 3, |r| { r.insert(VEC, Value::Vector(vec![5.0, 0.0])); });

    // Nearest to (4.5, 0) is row 3 (at 5,0), then row 1 (0,0).
    let res = m.vector_search(VEC, &[4.5, 0.0], 2, 64, m.snapshot_lsn());
    assert_eq!(res.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![3, 1]);

    // Snapshot just before the update (sees rows at their first versions).
    let snap_old = m.max_lsn();

    // Update row 3's vector to be far away; nearest becomes row 1.
    put(&mut m, 3, 4, |r| { r.insert(VEC, Value::Vector(vec![99.0, 0.0])); });
    let res = m.vector_search(VEC, &[4.5, 0.0], 1, 64, m.snapshot_lsn());
    assert_eq!(res[0].0, 1);

    // The earlier snapshot still sees the old vector (row 3 nearest).
    let res_old = m.vector_search(VEC, &[4.5, 0.0], 1, 64, snap_old);
    assert_eq!(res_old[0].0, 3);
}

#[test]
fn text_search_sees_recent_writes_and_deletes() {
    let mut m = Memtable::new();
    put(&mut m, 1, 1, |r| { r.insert(BODY, Value::Utf8("quick brown fox".into())); });
    put(&mut m, 2, 2, |r| { r.insert(BODY, Value::Utf8("quick quick volatility".into())); });
    put(&mut m, 3, 3, |r| { r.insert(BODY, Value::Utf8("lazy dog".into())); });

    let res = m.text_search(BODY, "quick", 10, m.snapshot_lsn());
    let ids: Vec<u64> = res.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&1) && ids.contains(&2) && !ids.contains(&3));
    // Row 2 mentions "quick" twice → should outrank row 1.
    assert_eq!(ids[0], 2);

    // Delete row 2; it disappears from results immediately.
    m.apply(2, Op::Delete, 4);
    let res = m.text_search(BODY, "quick", 10, m.snapshot_lsn());
    let ids: Vec<u64> = res.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![1]);
}

#[test]
fn graph_traversal_sees_recent_writes() {
    let mut m = Memtable::new();
    put(&mut m, 1, 1, |r| { r.insert(FOLLOWS, Value::Edges(vec![2, 3])); });
    put(&mut m, 2, 2, |r| { r.insert(FOLLOWS, Value::Edges(vec![3])); });
    put(&mut m, 3, 3, |r| { r.insert(FOLLOWS, Value::Edges(vec![])); });

    let snap = m.snapshot_lsn();
    assert_eq!(m.out_neighbors(FOLLOWS, 1, snap), vec![2, 3]);
    let mut into3 = m.in_neighbors(FOLLOWS, 3, snap);
    into3.sort();
    assert_eq!(into3, vec![1, 2]);

    // Deleting a target prunes it from forward neighbors (visibility-filtered).
    m.apply(2, Op::Delete, 4);
    assert_eq!(m.out_neighbors(FOLLOWS, 1, m.snapshot_lsn()), vec![3]);
}

#[test]
fn scalar_access_respects_snapshot() {
    let mut m = Memtable::new();
    put(&mut m, 1, 1, |r| { r.insert(9, Value::I64(100)); });
    put(&mut m, 1, 5, |r| { r.insert(9, Value::I64(200)); });
    assert_eq!(m.scalar(1, 9, m.snapshot_lsn()), Some(Value::I64(200)));
    assert_eq!(m.scalar(1, 9, 3), Some(Value::I64(100)));
}
