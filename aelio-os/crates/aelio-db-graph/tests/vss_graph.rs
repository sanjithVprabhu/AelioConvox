//! End-to-end: one `.vss` file holds a node id column AND the Edge index section.
//! Reopen and run forward/reverse traversal entirely from the file.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aelio_db_format::{
    read_file, write_file_with, Column, ColumnValues, FileMeta, MvccSummary, RawSection,
    SectionEncoding, SectionType, WriteOptions,
};
use aelio_db_graph::{serialize_edge_index, EdgeIndex, EdgeView};

const EDGE_COL: u32 = 40;
const ID_COL: u32 = 1;

static COUNTER: AtomicU64 = AtomicU64::new(0);
struct TempPath(PathBuf);
impl TempPath {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        TempPath(std::env::temp_dir().join(format!("vss_graph_{}_{n}.vss", std::process::id())))
    }
}
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn traversal_from_a_reopened_vss_file() {
    // A 6-node "follows" graph; global target ids equal local offsets in this single file.
    let edges = vec![(0u32, 1u64), (0, 3), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0)];
    let n = 6usize;
    let g = EdgeIndex::build(&edges, n);
    let edge_section = serialize_edge_index(&g, EDGE_COL);

    // A trivial scalar column so the file has rows.
    let id_col = Column {
        column_id: ID_COL,
        is_system: false,
        values: ColumnValues::I64((0..n as i64).map(Some).collect()),
    };

    let meta = FileMeta {
        file_uuid: *b"vss-graph-demo01",
        min_lsn: 1,
        max_lsn: n as u64,
        row_count: n as u64,
        schema_fingerprint: 0x6E0D,
        creation_unix_nanos: 0,
        schema_blob: b"users(follows ROW REF[])".to_vec(),
        mvcc: MvccSummary {
            min_xmin: 1,
            max_xmax: u64::MAX,
            tombstone_count: 0,
            live_count: n as u64,
        },
        writer_version: "aelio-db-graph-e2e".into(),
    };
    let translation: Vec<u64> = (0..n as u64).collect();
    let extra = [RawSection {
        kind: SectionType::Edge,
        column_id: EDGE_COL,
        encoding: SectionEncoding::Plain,
        bytes: edge_section.clone(),
    }];

    let path = TempPath::new();
    write_file_with(
        &path.0,
        &meta,
        &[id_col],
        &translation,
        &extra,
        &WriteOptions::default(),
    )
    .expect("write");

    // Reopen and traverse using only what is in the file.
    let file = read_file(&path.0).expect("read");
    let section = file
        .section(SectionType::Edge, EDGE_COL)
        .expect("edge section present");
    assert_eq!(section, edge_section.as_slice(), "edge section round-trips");
    let view = EdgeView::parse(section).expect("parse view");

    // Forward: who does node 0 follow (directly)?
    assert_eq!(view.out_neighbors(0), vec![1, 3]);
    // Reverse: who follows node 3?
    assert_eq!(view.in_neighbors(3), vec![0, 2]);

    // 2-hop reachability from node 0: {1,3} then their targets {2,4}.
    let resolve = |t: u64| Some(t as u32);
    let reach2 = view.traverse_forward(&[0], 2, resolve);
    assert!(
        reach2.contains(&1) && reach2.contains(&3) && reach2.contains(&2) && reach2.contains(&4)
    );
}
