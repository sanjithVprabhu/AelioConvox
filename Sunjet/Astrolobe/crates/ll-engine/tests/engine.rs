//! The write loop: durable writes, crash recovery from the WAL, and flush to a `.vss` file.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_engine::{Engine, Row, Value};
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
