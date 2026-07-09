//! Shared test helpers.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_format::{FileMeta, MvccSummary};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique temp path that cleans itself up on drop.
pub struct TempPath(pub PathBuf);

impl TempPath {
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("ll_{tag}_{pid}_{n}.vss"));
        TempPath(path)
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut tmp = self.0.clone();
        let mut name = tmp.file_name().unwrap().to_os_string();
        name.push(".tmp");
        tmp.set_file_name(name);
        let _ = std::fs::remove_file(&tmp);
    }
}

/// A representative `FileMeta` for a given row count.
pub fn sample_meta(row_count: u64) -> FileMeta {
    FileMeta {
        file_uuid: *b"0123456789abcdef",
        min_lsn: 1000,
        max_lsn: 1000 + row_count,
        row_count,
        schema_fingerprint: 0xDEAD_BEEF_CAFE_F00D,
        creation_unix_nanos: 1_700_000_000_000_000_000,
        schema_blob: b"documents(id,title,body,embedding,author)".to_vec(),
        mvcc: MvccSummary {
            min_xmin: 1000,
            max_xmax: u64::MAX,
            tombstone_count: 0,
            live_count: row_count,
        },
        writer_version: "ll-format/0.0.1".to_string(),
    }
}
