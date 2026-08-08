//! The write loop: durable writes, crash recovery from the WAL, and flush to a `.vss` file.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_engine::{Engine, Row, Value, WriteOp};
use ll_format::{read_file, ColumnValues};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Tmp(PathBuf);
impl Tmp {
    fn new(ext: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Tmp(std::env::temp_dir().join(format!("ll_eng_{}_{n}.{ext}", std::process::id())))
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn row(name: &str, score: i64) -> Row {
    let mut r = Row::new();
    r.insert(1, Value::Utf8(name.into()));
    r.insert(2, Value::I64(score));
    r
}

#[test]
fn writes_survive_recovery() {
    let wal = Tmp::new("log");

    // Session 1: write some rows, then "crash" (drop the engine).
    {
        let mut e = Engine::create(&wal.0, 1).unwrap();
        e.insert(10, row("alice", 100)).unwrap();
        e.insert(20, row("bob", 200)).unwrap();
        e.insert(30, row("carol", 300)).unwrap();
        e.delete(20).unwrap();
        assert_eq!(e.live_count(), 2);
    }

    // Session 2: recover from the WAL — committed writes are back, delete applied.
    {
        let mut e = Engine::recover(&wal.0).unwrap();
        assert_eq!(e.live_count(), 2);
        assert_eq!(e.get(10).unwrap().get(&1), Some(&Value::Utf8("alice".into())));
        assert!(e.get(20).is_none(), "deleted row stays deleted after recovery");
        assert_eq!(e.get(30).unwrap().get(&2), Some(&Value::I64(300)));

        // Continue writing after recovery.
        e.insert(40, row("dave", 400)).unwrap();
    }

    // Session 3: the post-recovery write is durable too.
    {
        let e = Engine::recover(&wal.0).unwrap();
        assert_eq!(e.live_count(), 3);
        assert_eq!(e.get(40).unwrap().get(&1), Some(&Value::Utf8("dave".into())));
    }
}

#[test]
fn committed_batch_is_visible_and_recovers_atomically() {
    let wal = Tmp::new("batch.log");
    {
        let mut e = Engine::create(&wal.0, 1).unwrap();
        let commit = e
            .transact(vec![
                WriteOp::Put { row_id: 10, row: row("alice", 100) },
                WriteOp::Put { row_id: 20, row: row("bob", 200) },
            ])
            .unwrap();
        assert!(commit > 0);
        assert_eq!(e.live_count(), 2);
    }
    let recovered = Engine::recover(&wal.0).unwrap();
    assert_eq!(recovered.live_count(), 2);
    assert_eq!(recovered.get(10).unwrap().get(&1), Some(&Value::Utf8("alice".into())));
    assert_eq!(recovered.get(20).unwrap().get(&2), Some(&Value::I64(200)));
}

/// A batch must appear at exactly one LSN. Stamping each row with its own record LSN would let a
/// reader whose snapshot falls between two records of the same transaction observe half of it —
/// and would make the LSN `transact` returns unusable as a read-your-writes snapshot.
#[test]
fn a_batch_is_invisible_at_every_snapshot_before_its_commit() {
    let wal = Tmp::new("atomic.log");
    let mut e = Engine::create(&wal.0, 1).unwrap();

    let first = e.transact(vec![WriteOp::Put { row_id: 1, row: row("seed", 1) }]).unwrap();
    let commit = e
        .transact(vec![
            WriteOp::Put { row_id: 10, row: row("alice", 100) },
            WriteOp::Put { row_id: 20, row: row("bob", 200) },
            WriteOp::Put { row_id: 30, row: row("carol", 300) },
        ])
        .unwrap();
    assert!(commit > first + 1, "the batch must span more than one WAL record");

    let mem = e.memtable();
    // Every snapshot strictly before the commit sees none of the batch, including snapshots that
    // sit between the batch's individual record LSNs.
    for snapshot in first..commit {
        for row_id in [10u64, 20, 30] {
            assert!(
                mem.get(row_id, snapshot).is_none(),
                "row {row_id} leaked at snapshot {snapshot} (commit was {commit})",
            );
        }
    }
    // At the commit LSN the whole batch is visible at once.
    for row_id in [10u64, 20, 30] {
        assert!(mem.get(row_id, commit).is_some(), "row {row_id} missing at the commit snapshot");
    }

    // Recovery must reproduce exactly the same visibility boundary.
    drop(e);
    let recovered = Engine::recover(&wal.0).unwrap();
    let mem = recovered.memtable();
    for row_id in [10u64, 20, 30] {
        assert!(mem.get(row_id, commit - 1).is_none(), "row {row_id} leaked after recovery");
        assert!(mem.get(row_id, commit).is_some(), "row {row_id} missing after recovery");
    }
}

#[test]
fn flush_writes_a_vss_file() {
    let wal = Tmp::new("log");
    let vss = Tmp::new("vss");

    let mut e = Engine::create(&wal.0, 1).unwrap();
    e.insert(1, row("x", 11)).unwrap();
    e.insert(2, row("y", 22)).unwrap();
    e.insert(3, row("z", 33)).unwrap();
    e.delete(2).unwrap();

    let n = e.flush_to(&vss.0).unwrap();
    assert_eq!(n, 3); // rows 1 and 3 are live; row 2 is persisted as a tombstone

    // Reopen the .vss file and verify the persisted columns. The deleted row 2 keeps its
    // slot (so its tombstone suppresses any older copy), with null data and xmax == xmin.
    let file = read_file(&vss.0).unwrap();
    assert_eq!(file.preamble.row_count, 3);
    assert_eq!(file.translation_table, vec![1, 2, 3]); // row ids in order, incl. tombstone

    let name_col = file.columns.iter().find(|c| c.column_id == 1).unwrap();
    match &name_col.values {
        ColumnValues::Utf8(v) => {
            assert_eq!(v, &vec![Some("x".to_string()), None, Some("z".to_string())]);
        }
        _ => panic!("expected utf8 name column"),
    }
    let score_col = file.columns.iter().find(|c| c.column_id == 2).unwrap();
    match &score_col.values {
        ColumnValues::I64(v) => assert_eq!(v, &vec![Some(11), None, Some(33)]),
        _ => panic!("expected i64 score column"),
    }

    // The tombstone is marked: row 2 (local offset 1) has a finite xmax; live rows don't.
    let xmax = file
        .columns
        .iter()
        .find(|c| c.column_id == ll_engine::SYS_XMAX_COL)
        .unwrap();
    match &xmax.values {
        ColumnValues::I64(v) => {
            assert_eq!(v[0], Some(-1)); // row 1 live: xmax = u64::MAX (-1 as i64)
            assert_ne!(v[1], Some(-1)); // row 2 tombstone: finite xmax
            assert_eq!(v[2], Some(-1)); // row 3 live
        }
        _ => panic!("expected i64 xmax column"),
    }
}
