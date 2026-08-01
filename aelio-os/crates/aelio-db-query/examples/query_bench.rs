//! Scaled filtered-vector benchmark through the FULL query path.
//!
//! Run: `cargo run --release -p aelio-db-query --example query_bench`
//!
//! Builds a `.vss` file (vector column + scalar category + HNSW section) at scale, then for
//! a sweep of filter selectivities compares, against exact filtered ground truth:
//! OURS = the unified executor (cost model picks pre/post-filter automatically);
//! NAIVE = HNSW top-k then filter (what a vector DB + WHERE clause does).
//!
//! The headline is recall: OURS stays correct across all selectivities; NAIVE collapses
//! when the filter is selective.
//!
//! Honest caveats (v0): single-threaded, no SIMD, software CRC; and the executor estimates
//! selectivity with a bounded-memory sample that still makes one linear pass for the exact
//! visible-row count. Persisted per-column statistics will remove that planning pass.

use std::time::Instant;

use aelio_db_engine::SYS_XMIN_COL;
use aelio_db_format::{
    read_file, write_file_with, Column, ColumnValues, FileMeta, MvccSummary, RawSection,
    SectionEncoding, SectionType, WriteOptions,
};
use aelio_db_index::{serialize_hnsw, Hnsw, HnswParams};
use aelio_db_query::{execute, explain_plan, FileSource, PredOp, Query, Source, Value};

const N: usize = 20_000;
const DIM: usize = 32;
const K: usize = 10;
const QUERIES: usize = 30;
const VEC: u32 = 1;
const CAT: u32 = 2; // category in 0..1000
const SNAP: u64 = u64::MAX - 1;

struct Rng(u64);
impl Rng {
    fn new(s: u64) -> Self {
        Rng(s ^ 0x9E37_79B9)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 11) as f32 / (1u64 << 53) as f32
    }
}

fn l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

fn main() {
    eprintln!("building {N} x {DIM}d vectors + HNSW + .vss file ...");
    let mut rng = Rng::new(1);
    let data: Vec<Vec<f32>> = (0..N)
        .map(|_| (0..DIM).map(|_| rng.f()).collect())
        .collect();
    let cats: Vec<i64> = (0..N).map(|_| (rng.next() % 1000) as i64).collect();

    // Build the HNSW and the .vss file directly (skip the WAL fsync-per-row write path).
    let t = Instant::now();
    let mut hnsw = Hnsw::new(DIM, HnswParams::default());
    for v in &data {
        hnsw.insert(v.clone());
    }
    hnsw.quantize();
    let local_offsets: Vec<u32> = (0..N as u32).collect();
    let hnsw_bytes = serialize_hnsw(&hnsw, VEC, &local_offsets);
    eprintln!("  HNSW built in {:.1}s", t.elapsed().as_secs_f64());

    let columns = vec![
        Column {
            column_id: VEC,
            is_system: false,
            values: ColumnValues::Vector {
                dim: DIM as u16,
                data: data.iter().map(|v| Some(v.clone())).collect(),
            },
        },
        Column {
            column_id: CAT,
            is_system: false,
            values: ColumnValues::I64(cats.iter().map(|&c| Some(c)).collect()),
        },
        Column {
            column_id: SYS_XMIN_COL,
            is_system: true,
            values: ColumnValues::I64(vec![Some(1); N]),
        },
    ];
    let meta = FileMeta {
        file_uuid: [0; 16],
        min_lsn: 1,
        max_lsn: 1,
        row_count: N as u64,
        schema_fingerprint: 0,
        creation_unix_nanos: 0,
        schema_blob: Vec::new(),
        mvcc: MvccSummary {
            min_xmin: 1,
            max_xmax: u64::MAX,
            tombstone_count: 0,
            live_count: N as u64,
        },
        writer_version: "bench".into(),
    };
    let extra = [RawSection {
        kind: SectionType::Hnsw,
        column_id: VEC,
        encoding: SectionEncoding::Plain,
        bytes: hnsw_bytes,
    }];
    let path = std::env::temp_dir().join(format!("aelio_qbench_{}.vss", std::process::id()));
    write_file_with(
        &path,
        &meta,
        &columns,
        &local_offsets.iter().map(|&x| x as u64).collect::<Vec<_>>(),
        &extra,
        &WriteOptions::default(),
    )
    .unwrap();
    let file = read_file(&path).unwrap();
    let fs = FileSource::new(std::sync::Arc::new(file));
    let sources: &[&dyn Source] = &[&fs];

    let queries: Vec<Vec<f32>> = (0..QUERIES)
        .map(|_| (0..DIM).map(|_| rng.f()).collect())
        .collect();
    let sels = [0.005f64, 0.02, 0.05, 0.1, 0.25, 0.5, 1.0];

    println!(
        "\n{:>6} {:>8}  {:<10} {:>10} {:>9}   {:>12} {:>9}",
        "sel", "matched", "strategy", "OUR_recall", "OUR_ms", "NAIVE_recall", "NAIVE_ms"
    );
    println!("{}", "-".repeat(78));

    for &sel in &sels {
        let thr = (sel * 1000.0) as i64;
        let pred = |id: usize| cats[id] < thr;
        let matched = (0..N).filter(|&i| pred(i)).count();

        let q_template = |qv: &[f32]| {
            Query::new(K)
                .with_vector(VEC, qv.to_vec())
                .filter(CAT, PredOp::Lt, Value::I64(thr))
        };
        let strategy =
            if explain_plan(sources, &q_template(&queries[0]), SNAP).contains("PreFilter") {
                "PreFilter"
            } else {
                "PostFilter"
            };

        let mut our_recall = 0.0;
        let mut our_nanos = 0u128;
        let mut naive_recall = 0.0;
        let mut naive_nanos = 0u128;

        for qv in &queries {
            // exact filtered ground truth
            let mut truth: Vec<(f32, u64)> = (0..N)
                .filter(|&i| pred(i))
                .map(|i| (l2(qv, &data[i]), i as u64))
                .collect();
            truth.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let truth_set: std::collections::HashSet<u64> =
                truth.into_iter().take(K).map(|(_, i)| i).collect();
            if truth_set.is_empty() {
                continue;
            }

            // OURS: full executor (cost model chooses strategy)
            let q = q_template(qv);
            let start = Instant::now();
            let res = execute(sources, &q, SNAP);
            our_nanos += start.elapsed().as_nanos();
            let hits = res.iter().filter(|(id, _)| truth_set.contains(id)).count();
            our_recall += hits as f64 / truth_set.len() as f64;

            // NAIVE: HNSW top-(K*10) then filter, take top-K (what a vector DB + WHERE does)
            let start = Instant::now();
            let cands = fs.vector_search(VEC, qv, K * 10, 640, SNAP);
            let naive: Vec<u64> = cands
                .into_iter()
                .filter(|(id, _)| pred(*id as usize))
                .take(K)
                .map(|(id, _)| id)
                .collect();
            naive_nanos += start.elapsed().as_nanos();
            let hits = naive.iter().filter(|id| truth_set.contains(id)).count();
            naive_recall += hits as f64 / truth_set.len() as f64;
        }

        let nq = queries.len() as f64;
        println!(
            "{:>6.3} {:>8} {:<10}  {:>9.3} {:>8.2} {:>13.3} {:>8.2}",
            sel,
            matched,
            strategy,
            our_recall / nq,
            our_nanos as f64 / nq / 1e6,
            naive_recall / nq,
            naive_nanos as f64 / nq / 1e6,
        );
    }

    let _ = std::fs::remove_file(&path);
}
