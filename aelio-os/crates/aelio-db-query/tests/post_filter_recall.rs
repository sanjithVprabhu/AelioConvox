//! Regression guard: post-filter must over-fetch by selectivity so recall stays high in
//! the post-filter regime (a fixed k*4 fetch under-fetches at moderate selectivity → the
//! recall drop the scaled benchmark exposed). Uses a real HNSW (file) at moderate scale.

use aelio_db_engine::SYS_XMIN_COL;
use aelio_db_format::{
    read_file, write_file_with, Column, ColumnValues, FileMeta, MvccSummary, RawSection,
    SectionEncoding, SectionType, WriteOptions,
};
use aelio_db_index::{serialize_hnsw, Hnsw, HnswParams};
use aelio_db_query::{execute, FileSource, PredOp, Query, Source, Value};

const N: usize = 1500;
const DIM: usize = 12;
const VEC: u32 = 1;
const CAT: u32 = 2;
const SNAP: u64 = u64::MAX - 1;

struct Rng(u64);
impl Rng {
    fn new(s: u64) -> Self {
        Rng(s ^ 0x51ED)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z ^ (z >> 31)
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 11) as f32 / (1u64 << 53) as f32
    }
}

fn l2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

#[test]
fn post_filter_keeps_high_recall_at_moderate_selectivity() {
    let mut rng = Rng::new(1);
    let data: Vec<Vec<f32>> = (0..N)
        .map(|_| (0..DIM).map(|_| rng.f()).collect())
        .collect();
    let cats: Vec<i64> = (0..N).map(|_| (rng.next() % 100) as i64).collect();

    let mut h = Hnsw::new(DIM, HnswParams::default());
    for v in &data {
        h.insert(v.clone());
    }
    h.quantize();
    let offs: Vec<u32> = (0..N as u32).collect();
    let hnsw_bytes = serialize_hnsw(&h, VEC, &offs);

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
        writer_version: "t".into(),
    };
    let extra = [RawSection {
        kind: SectionType::Hnsw,
        column_id: VEC,
        encoding: SectionEncoding::Plain,
        bytes: hnsw_bytes,
    }];
    let path = std::env::temp_dir().join(format!("aelio_pf_{}.vss", std::process::id()));
    let tt: Vec<u64> = (0..N as u64).collect();
    write_file_with(
        &path,
        &meta,
        &columns,
        &tt,
        &extra,
        &WriteOptions::default(),
    )
    .unwrap();
    let file = read_file(&path).unwrap();
    let fs = FileSource::new(std::sync::Arc::new(file));
    let sources: &[&dyn Source] = &[&fs];

    // selectivity ~0.30 (cat < 30) → the cost model chooses PostFilter; recall must stay high.
    let thr = 30i64;
    let k = 10;
    let mut total_recall = 0.0;
    let queries: Vec<Vec<f32>> = (0..20)
        .map(|_| (0..DIM).map(|_| rng.f()).collect())
        .collect();
    for qv in &queries {
        let mut truth: Vec<(f32, u64)> = (0..N)
            .filter(|&i| cats[i] < thr)
            .map(|i| (l2(qv, &data[i]), i as u64))
            .collect();
        truth.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let set: std::collections::HashSet<u64> =
            truth.into_iter().take(k).map(|(_, i)| i).collect();

        let q = Query::new(k)
            .with_vector(VEC, qv.clone())
            .filter(CAT, PredOp::Lt, Value::I64(thr));
        let got = execute(sources, &q, SNAP);
        let hits = got.iter().filter(|(id, _)| set.contains(id)).count();
        total_recall += hits as f64 / set.len() as f64;
    }
    let recall = total_recall / queries.len() as f64;
    let _ = std::fs::remove_file(&path);
    assert!(
        recall >= 0.95,
        "post-filter recall at ~0.3 selectivity was {recall:.3}"
    );
}
