//! Reproduction: a row flushed to a `.vss` segment and then deleted (or updated) in the
//! memtable must NOT resurface from the older segment. This is the cross-source
//! "newest visible version wins / tombstone suppresses older files" guarantee.

use ll_engine::{Engine, Row, Value};
use ll_format::read_file;
use ll_query::{execute, FileSource, Query, Source};

const VEC: u32 = 1;
const SNAP: u64 = u64::MAX - 1;

#[test]
fn deleted_after_flush_does_not_resurface_from_segment() {
    let dir = std::env::temp_dir().join(format!("ll_xsrc_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let wal = dir.join("wal.log");
    let seg = dir.join("seg.vss");

    let mut engine = Engine::create(&wal, 1).unwrap();
    // Two rows, both flushed to the segment.
    let mut r1 = Row::new();
    r1.insert(VEC, Value::Vector(vec![0.0, 0.0]));
    engine.insert(42, r1).unwrap();
    let mut r2 = Row::new();
    r2.insert(VEC, Value::Vector(vec![10.0, 0.0]));
    engine.insert(7, r2).unwrap();
    engine.flush_to(&seg).unwrap();
    engine.reset_memtable();

    // Now delete row 42 — it lives only in the segment at this point.
    engine.delete(42).unwrap();

    let file = read_file(&seg).unwrap();
    let fs = FileSource::new(std::sync::Arc::new(file));
    let sources: Vec<&dyn Source> = vec![engine.memtable(), &fs];

    // Nearest to (0,0) is row 42 — but it's deleted, so it must not appear.
    let res = execute(&sources, &Query::new(2).with_vector(VEC, vec![0.0, 0.0]), SNAP);
    let ids: Vec<u64> = res.iter().map(|(id, _)| *id).collect();

    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !ids.contains(&42),
        "deleted-after-flush row 42 resurfaced from the segment: {ids:?}"
    );
}

#[test]
fn updated_after_flush_uses_the_new_version_not_the_stale_segment_copy() {
    let dir = std::env::temp_dir().join(format!("ll_xupd_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let wal = dir.join("wal.log");
    let seg = dir.join("seg.vss");

    let mut engine = Engine::create(&wal, 1).unwrap();
    let mut r1 = Row::new();
    r1.insert(VEC, Value::Vector(vec![0.0, 0.0]));
    engine.insert(42, r1).unwrap();
    let mut r2 = Row::new();
    r2.insert(VEC, Value::Vector(vec![10.0, 0.0]));
    engine.insert(7, r2).unwrap();
    engine.flush_to(&seg).unwrap();
    engine.reset_memtable();

    // Move row 42 far away. The segment still holds its old vector at (0,0).
    let mut r1b = Row::new();
    r1b.insert(VEC, Value::Vector(vec![100.0, 0.0]));
    engine.insert(42, r1b).unwrap();

    let file = read_file(&seg).unwrap();
    let fs = FileSource::new(std::sync::Arc::new(file));
    let sources: Vec<&dyn Source> = vec![engine.memtable(), &fs];

    // Nearest to (0,0): row 42 is now at (100,0), so row 7 (at 10,0) must win — the stale
    // (0,0) copy in the segment must not rank 42 first.
    let res = execute(&sources, &Query::new(1).with_vector(VEC, vec![0.0, 0.0]), SNAP);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(res.first().map(|(id, _)| *id), Some(7), "got {res:?}");
}

#[test]
fn delete_survives_a_second_flush() {
    // The tombstone must be persisted into the new segment, or dropping it at flush would
    // resurrect the row's still-live copy in the older segment.
    let dir = std::env::temp_dir().join(format!("ll_x2f_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let wal = dir.join("wal.log");
    let seg0 = dir.join("seg0.vss");
    let seg1 = dir.join("seg1.vss");

    let mut engine = Engine::create(&wal, 1).unwrap();
    let mut r1 = Row::new();
    r1.insert(VEC, Value::Vector(vec![0.0, 0.0]));
    engine.insert(42, r1).unwrap();
    engine.flush_to(&seg0).unwrap();
    engine.reset_memtable();

    // Delete, then flush again — the second segment carries the tombstone.
    engine.delete(42).unwrap();
    engine.flush_to(&seg1).unwrap();
    engine.reset_memtable();

    let f0 = read_file(&seg0).unwrap();
    let f1 = read_file(&seg1).unwrap();
    let fs0 = FileSource::new(std::sync::Arc::new(f0));
    let fs1 = FileSource::new(std::sync::Arc::new(f1));
    let sources: Vec<&dyn Source> = vec![engine.memtable(), &fs0, &fs1];

    let res = execute(&sources, &Query::new(2).with_vector(VEC, vec![0.0, 0.0]), SNAP);
    let ids: Vec<u64> = res.iter().map(|(id, _)| *id).collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !ids.contains(&42),
        "row 42 resurrected after its tombstone was flushed to a second segment: {ids:?}"
    );
}
