//! Segment-store lifecycle tests (local + simulated cloud).

use std::sync::Arc;

use ll_catalog::ColumnKind;
use ll_engine::Value;
use ll_query::Database;
use ll_storage::{CachedSegmentStore, LocalSegmentStore, MemorySegmentStore, SegmentStore};

#[test]
fn flush_publishes_to_memory_and_reopens_via_download() {
    let dir = tempfile::tempdir().unwrap();
    let remote = Arc::new(MemorySegmentStore::new());
    let local = LocalSegmentStore::new(dir.path());
    let store: Arc<dyn SegmentStore> =
        Arc::new(CachedSegmentStore::new(local, Arc::clone(&remote) as Arc<dyn SegmentStore>));

    {
        let mut db = Database::create_with_store(dir.path().to_path_buf(), Arc::clone(&store)).unwrap();
        db.create_table("t", &[("name", ColumnKind::Utf8), ("n", ColumnKind::I64)])
            .unwrap();
        db.insert("t", &[("name", Value::Utf8("alpha".into())), ("n", Value::I64(1))])
            .unwrap();
        db.insert("t", &[("name", Value::Utf8("beta".into())), ("n", Value::I64(2))])
            .unwrap();
        db.flush().unwrap();
    }

    assert!(remote.contains("seg-00000.vss"), "segment must be uploaded to remote");

    // Evict the local segment so open must re-fetch from "cloud".
    let seg = dir.path().join("seg-00000.vss");
    assert!(seg.exists());
    std::fs::remove_file(&seg).unwrap();
    assert!(!seg.exists());

    let db = Database::open_with_store(dir.path().to_path_buf(), store).unwrap();
    assert!(seg.exists(), "ensure_local should rehydrate the segment");
    let rows = db
        .scan_values(
            "t",
            &ll_query::HybridQuery::new(10),
        )
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn compact_prunes_old_segments_from_remote() {
    let dir = tempfile::tempdir().unwrap();
    let remote = Arc::new(MemorySegmentStore::new());
    let local = LocalSegmentStore::new(dir.path());
    let store: Arc<dyn SegmentStore> =
        Arc::new(CachedSegmentStore::new(local, Arc::clone(&remote) as Arc<dyn SegmentStore>));

    let mut db = Database::create_with_store(dir.path().to_path_buf(), store).unwrap();
    db.create_table("t", &[("name", ColumnKind::Utf8)]).unwrap();

    db.insert("t", &[("name", Value::Utf8("one".into()))]).unwrap();
    db.flush().unwrap();
    assert!(remote.contains("seg-00000.vss"));

    db.insert("t", &[("name", Value::Utf8("two".into()))]).unwrap();
    db.flush().unwrap();
    assert!(remote.contains("seg-00001.vss"));

    db.compact().unwrap();

    // After compact, only the new merged segment should remain in remote.
    assert!(!remote.contains("seg-00000.vss"), "old segment pruned from cloud");
    assert!(!remote.contains("seg-00001.vss"), "old segment pruned from cloud");
    assert!(remote.contains("seg-00002.vss") || remote.len() == 1);

    let rows = db.scan_values("t", &ll_query::HybridQuery::new(10)).unwrap();
    assert_eq!(rows.len(), 2);
}
