//! End-to-end: one `.vss` file holds the text column (Utf8) AND the Text index section.
//! Reopen and run a BM25 query entirely from the file.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aelio_db_format::{
    read_file, write_file_with, Column, ColumnValues, FileMeta, MvccSummary, RawSection,
    SectionEncoding, SectionType, WriteOptions,
};
use aelio_db_text::{serialize_text_index, InvertedIndex, TextView};

const BODY_COL: u32 = 30;

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TempPath(PathBuf);
impl TempPath {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        TempPath(std::env::temp_dir().join(format!("vss_text_{}_{n}.vss", std::process::id())))
    }
}
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn bm25_from_a_reopened_vss_file() {
    let docs = [
        Some("the quick brown fox".to_string()),
        Some("quick derivatives and volatility".to_string()),
        Some("volatility hedging strategies".to_string()),
        None,
        Some("a lazy brown dog".to_string()),
    ];
    let n = docs.len();

    // Build the inverted index, serialize the Text section.
    let doc_refs: Vec<Option<&str>> = docs.iter().map(|d| d.as_deref()).collect();
    let idx = InvertedIndex::build(&doc_refs);
    let text_section = serialize_text_index(&idx, BODY_COL);

    // The text column itself (Utf8) lives in the file too.
    let body_col = Column {
        column_id: BODY_COL,
        is_system: false,
        values: ColumnValues::Utf8(docs.to_vec()),
    };

    let meta = FileMeta {
        file_uuid: *b"vss-text-demo-01",
        min_lsn: 1,
        max_lsn: n as u64,
        row_count: n as u64,
        schema_fingerprint: 0x7E47,
        creation_unix_nanos: 0,
        schema_blob: b"docs(body TEXT)".to_vec(),
        mvcc: MvccSummary {
            min_xmin: 1,
            max_xmax: u64::MAX,
            tombstone_count: 0,
            live_count: n as u64,
        },
        writer_version: "aelio-db-text-e2e".into(),
    };
    let translation: Vec<u64> = (0..n as u64).collect();
    let extra = [RawSection {
        kind: SectionType::Text,
        column_id: BODY_COL,
        encoding: SectionEncoding::Plain,
        bytes: text_section.clone(),
    }];

    let path = TempPath::new();
    write_file_with(
        &path.0,
        &meta,
        &[body_col],
        &translation,
        &extra,
        &WriteOptions::default(),
    )
    .expect("write");

    // Reopen and query using only what is in the file.
    let file = read_file(&path.0).expect("read");
    let section = file
        .section(SectionType::Text, BODY_COL)
        .expect("text section present");
    assert_eq!(section, text_section.as_slice(), "text section round-trips");
    let view = TextView::parse(section).expect("parse view");

    // "volatility" is in docs 1 and 2; doc 2 is shorter → should rank first.
    let hits = view.bm25("volatility", 10);
    let ranked: Vec<u32> = hits.iter().map(|(d, _)| *d).collect();
    assert_eq!(&ranked[..2], &[2, 1], "BM25 ranking from file: {ranked:?}");

    // The Utf8 column round-trips so the engine can fetch the matched documents.
    let read_body = match &file.columns[0].values {
        ColumnValues::Utf8(v) => v.clone(),
        _ => panic!("expected utf8 column"),
    };
    assert_eq!(read_body, docs.to_vec());
}
