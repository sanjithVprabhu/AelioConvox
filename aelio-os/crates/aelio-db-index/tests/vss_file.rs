//! End-to-end: one `.vss` file holds the vector column, a scalar filter column, AND the
//! HNSW index section. Reopen the file and run a filtered vector search entirely from it.
//! This closes the storage loop: `aelio-db-index` produces the index bytes, `aelio-db-format` frames
//! them inside the file, and the reopened file rebuilds a searchable view.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use aelio_db_format::{
    read_file, write_file_with, Column as FmtColumn, ColumnValues, FileMeta, MvccSummary,
    RawSection, SectionEncoding, SectionType, WriteOptions,
};
use aelio_db_index::{
    distance, integrated, serialize_hnsw, Hnsw, HnswParams, HnswView, Metric, SplitMix64,
};

const VEC_COL: u32 = 10;
const FILT_COL: u32 = 20;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempPath(PathBuf);
impl TempPath {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        TempPath(std::env::temp_dir().join(format!("vss_e2e_{}_{n}.vss", std::process::id())))
    }
}
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn gen(seed: u64, n: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| (0..dim).map(|_| rng.next_f64() as f32).collect())
        .collect()
}

#[test]
fn filtered_search_from_a_reopened_vss_file() {
    let n = 1000usize;
    let dim = 16usize;
    let k = 10usize;
    let ef = 64usize;
    let metric = Metric::L2;

    // --- build the index and the columns in memory ---
    let data = gen(1, n, dim);
    let mut index = Hnsw::new(
        dim,
        HnswParams {
            m: 16,
            ef_construction: 100,
            seed: 42,
            metric,
        },
    );
    for v in &data {
        index.insert(v.clone());
    }
    index.quantize();
    let local_offsets: Vec<u32> = (0..n as u32).collect();
    let translation: Vec<u64> = (0..n as u64).collect();
    let hnsw_bytes = serialize_hnsw(&index, VEC_COL, &local_offsets);

    let filt: Vec<f32> = gen(7, n, 1).into_iter().map(|v| v[0]).collect();

    let vec_col = FmtColumn {
        column_id: VEC_COL,
        is_system: false,
        values: ColumnValues::Vector {
            dim: dim as u16,
            data: data.iter().map(|v| Some(v.clone())).collect(),
        },
    };
    let filt_col = FmtColumn {
        column_id: FILT_COL,
        is_system: false,
        values: ColumnValues::F32(filt.iter().map(|&x| Some(x)).collect()),
    };

    let meta = FileMeta {
        file_uuid: *b"vss-e2e-demo-001",
        min_lsn: 1,
        max_lsn: n as u64,
        row_count: n as u64,
        schema_fingerprint: 0xABCD,
        creation_unix_nanos: 0,
        schema_blob: b"docs(embedding,filter)".to_vec(),
        mvcc: MvccSummary {
            min_xmin: 1,
            max_xmax: u64::MAX,
            tombstone_count: 0,
            live_count: n as u64,
        },
        writer_version: "aelio-db-index-e2e".into(),
    };

    // --- write ONE .vss file with both columns + the HNSW section ---
    let path = TempPath::new();
    let extra = [RawSection {
        kind: SectionType::Hnsw,
        column_id: VEC_COL,
        encoding: SectionEncoding::Plain,
        bytes: hnsw_bytes.clone(),
    }];
    write_file_with(
        &path.0,
        &meta,
        &[vec_col, filt_col],
        &translation,
        &extra,
        &WriteOptions::default(),
    )
    .expect("write");

    // --- reopen and reconstruct everything from the file alone ---
    let file = read_file(&path.0).expect("read");

    // vector column f32 (for rerank)
    let read_vecs: Vec<Vec<f32>> = match &file
        .columns
        .iter()
        .find(|c| c.column_id == VEC_COL)
        .unwrap()
        .values
    {
        ColumnValues::Vector { data, .. } => data.iter().map(|x| x.clone().unwrap()).collect(),
        _ => panic!("expected vector column"),
    };
    assert_eq!(read_vecs, data, "vector column round-trips");

    // filter column (for the predicate)
    let read_filt: Vec<f32> = match &file
        .columns
        .iter()
        .find(|c| c.column_id == FILT_COL)
        .unwrap()
        .values
    {
        ColumnValues::F32(v) => v.iter().map(|x| x.unwrap()).collect(),
        _ => panic!("expected f32 column"),
    };

    // HNSW section bytes (for the index) — and confirm they survived intact
    let section = file
        .section(SectionType::Hnsw, VEC_COL)
        .expect("hnsw section present");
    assert_eq!(section, hnsw_bytes.as_slice(), "hnsw section round-trips");
    let view = HnswView::parse(section).expect("parse view from file");

    // --- run a filtered vector search using only what we read back ---
    let s = 0.2f32;
    let predicate = |id: u32| read_filt[id as usize] < s;
    let queries = gen(999, 20, dim);

    let mut total_recall = 0.0;
    for q in &queries {
        // ground truth: exact top-k among matching rows, using the file's own vectors
        let mut truth: Vec<(f32, u32)> = read_vecs
            .iter()
            .enumerate()
            .filter(|(i, _)| predicate(*i as u32))
            .map(|(i, v)| (distance(metric, q, v), i as u32))
            .collect();
        truth.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let truth_set: std::collections::HashSet<u32> =
            truth.into_iter().take(k).map(|(_, i)| i).collect();

        let res = integrated(&view, q, &read_vecs, predicate, k, ef, metric);
        let hits = res
            .results
            .iter()
            .filter(|(id, _)| truth_set.contains(id))
            .count();
        total_recall += hits as f64 / truth_set.len().max(1) as f64;
    }
    let recall = total_recall / queries.len() as f64;
    assert!(
        recall >= 0.95,
        "end-to-end filtered recall {recall:.3} too low"
    );
}
