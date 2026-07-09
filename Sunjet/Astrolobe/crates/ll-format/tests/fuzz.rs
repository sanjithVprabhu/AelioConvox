//! Robustness: `read_file` must never panic on malformed input — random bytes and
//! arbitrarily-mutated valid files must all return `Ok`/`Err`, never crash. A reader that
//! panics on bad bytes is a denial-of-service and a correctness hole.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use ll_format::{read_file, write_file, Column, ColumnValues, FileMeta, MvccSummary};

struct Rng(u64);
impl Rng {
    fn new(s: u64) -> Self {
        Rng(s ^ 0x243F_6A88_85A3_08D3)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn tmp(ext: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ll_fuzz_{}_{n}.{ext}", std::process::id()))
}

fn valid_file_bytes() -> Vec<u8> {
    let path = tmp("vss");
    let meta = FileMeta {
        file_uuid: *b"fuzz-seed-file01",
        min_lsn: 1,
        max_lsn: 5,
        row_count: 3,
        schema_fingerprint: 1,
        creation_unix_nanos: 0,
        schema_blob: b"t(a,b)".to_vec(),
        mvcc: MvccSummary { min_xmin: 1, max_xmax: u64::MAX, tombstone_count: 0, live_count: 3 },
        writer_version: "fuzz".into(),
    };
    let columns = vec![
        Column {
            column_id: 1,
            is_system: false,
            values: ColumnValues::I64(vec![Some(1), Some(2), Some(3)]),
        },
        Column {
            column_id: 2,
            is_system: false,
            values: ColumnValues::Utf8(vec![Some("x".into()), None, Some("zzz".into())]),
        },
    ];
    write_file(&path, &meta, &columns, &[10, 20, 30]).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    bytes
}

#[test]
fn read_file_never_panics_on_random_bytes() {
    let mut r = Rng::new(1);
    for _ in 0..1000 {
        let len = r.below(256) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| r.below(256) as u8).collect();
        let p = tmp("bin");
        std::fs::write(&p, &bytes).unwrap();
        // Must return without panicking (result is intentionally ignored).
        let _ = read_file(&p);
        let _ = std::fs::remove_file(&p);
    }
}

#[test]
fn read_file_never_panics_on_mutated_valid_files() {
    let base = valid_file_bytes();
    let mut r = Rng::new(2);
    for _ in 0..2000 {
        let mut bytes = base.clone();
        // flip 1–4 random bytes
        let flips = 1 + r.below(4);
        for _ in 0..flips {
            let idx = r.below(bytes.len() as u64) as usize;
            bytes[idx] ^= (1 + r.below(255)) as u8;
        }
        let p = tmp("bin");
        std::fs::write(&p, &bytes).unwrap();
        let _ = read_file(&p);
        let _ = std::fs::remove_file(&p);
    }
}

#[test]
fn read_file_never_panics_on_truncation() {
    let base = valid_file_bytes();
    // Every possible prefix length must be handled gracefully.
    for len in 0..base.len() {
        let p = tmp("bin");
        std::fs::write(&p, &base[..len]).unwrap();
        let _ = read_file(&p);
        let _ = std::fs::remove_file(&p);
    }
}
