//! Catalog CRUD, column-id assignment, file registration, and persistence round-trip.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_catalog::{Catalog, CatalogError, ColumnKind, FileRef};

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Tmp(std::env::temp_dir().join(format!("ll_cat_{}_{n}.cat", std::process::id())))
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn docs_schema() -> Vec<(&'static str, ColumnKind)> {
    vec![
        ("title", ColumnKind::Utf8),
        ("body", ColumnKind::Text),
        ("embedding", ColumnKind::Vector(1536)),
        ("cites", ColumnKind::Edge),
        ("created_at", ColumnKind::Timestamp),
    ]
}

#[test]
fn create_and_inspect_table() {
    let mut cat = Catalog::new();
    let tid = cat.create_table("documents", &docs_schema()).unwrap();
    let t = cat.table("documents").unwrap();
    assert_eq!(t.table_id, tid);
    assert_eq!(t.columns.len(), 5);

    // Column ids assigned 1..=5 in order.
    assert_eq!(t.column("title").unwrap().column_id, 1);
    assert_eq!(t.column("embedding").unwrap().kind, ColumnKind::Vector(1536));
    assert!(t.column("body").unwrap().kind.is_indexed_modality());
    assert!(!t.column("title").unwrap().kind.is_indexed_modality());

    // Duplicate name rejected.
    assert_eq!(
        cat.create_table("documents", &docs_schema()).unwrap_err(),
        CatalogError::TableExists("documents".into())
    );
}

#[test]
fn register_and_list_files() {
    let mut cat = Catalog::new();
    let tid = cat.create_table("t", &[("x", ColumnKind::I64)]).unwrap();
    cat.register_file(tid, FileRef { path: "a.vss".into(), min_lsn: 1, max_lsn: 9, row_count: 100 }).unwrap();
    cat.register_file(tid, FileRef { path: "b.vss".into(), min_lsn: 10, max_lsn: 20, row_count: 50 }).unwrap();
    assert_eq!(cat.files(tid).len(), 2);
    assert_eq!(cat.files(tid)[1].path, "b.vss");

    // Registering under an unknown table errors.
    assert!(cat.register_file(999, FileRef { path: "x".into(), min_lsn: 0, max_lsn: 0, row_count: 0 }).is_err());
}

#[test]
fn persistence_roundtrip() {
    let tmp = Tmp::new();
    let mut cat = Catalog::new();
    let tid = cat.create_table("documents", &docs_schema()).unwrap();
    cat.register_file(tid, FileRef { path: "docs-0.vss".into(), min_lsn: 1, max_lsn: 42, row_count: 1000 }).unwrap();
    cat.create_table("users", &[("name", ColumnKind::Utf8), ("vec", ColumnKind::Vector(8))]).unwrap();

    cat.save(&tmp.0).unwrap();
    let loaded = Catalog::load(&tmp.0).unwrap();
    assert_eq!(loaded, cat);

    // New table ids continue past the loaded max.
    let mut loaded = loaded;
    let id = loaded.create_table("third", &[("a", ColumnKind::Bool)]).unwrap();
    assert_eq!(id, 3);
}
