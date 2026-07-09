//! LL's runner for the shared filtered-vector benchmark.
//!
//! Reads a dataset in the common format (see `bench/BENCHMARKS.md`) — generating a
//! synthetic one if absent — builds the LL index, runs filtered top-k queries across a
//! selectivity sweep, and writes `<dir>/results/ll.json` plus a printed table. Competitor
//! runners (`bench/*.py`) read the *same* dataset and emit the *same* JSON schema, so
//! `bench/compare.py` can put everyone side by side.
//!
//! Usage: `ll-bench [dir] [N] [dim] [Q]`  (defaults: ./benchdata 10000 32 50)

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use ll_engine::SYS_XMIN_COL;
use ll_format::{
    read_file, write_file_with, Column, ColumnValues, FileMeta, MvccSummary, RawSection,
    SectionEncoding, SectionType, WriteOptions,
};
use ll_index::{encode_recall_curve, serialize_hnsw, Hnsw, HnswParams, RECALL_EF_LADDER};
use ll_query::{execute, explain_plan, FileSource, PredOp, Query, Source, Value};

const VEC: u32 = 1;
const CAT: u32 = 2;
const SNAP: u64 = u64::MAX - 1;
const K: usize = 10;
const SELS: [f64; 7] = [0.005, 0.02, 0.05, 0.1, 0.25, 0.5, 1.0];

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

// ---- shared dataset format (little-endian) ----
// base.fvecs / query.fvecs: repeated [dim: i32][dim x f32].  attr.u32: [N x u32].

fn write_fvecs(path: &Path, vecs: &[Vec<f32>]) {
    let mut buf = Vec::new();
    for v in vecs {
        buf.extend_from_slice(&(v.len() as i32).to_le_bytes());
        for &x in v {
            buf.extend_from_slice(&x.to_le_bytes());
        }
    }
    fs::write(path, buf).unwrap();
}

fn read_fvecs(path: &Path) -> Vec<Vec<f32>> {
    let mut b = Vec::new();
    fs::File::open(path).unwrap().read_to_end(&mut b).unwrap();
    let mut out = Vec::new();
    let mut p = 0;
    while p + 4 <= b.len() {
        let dim = i32::from_le_bytes(b[p..p + 4].try_into().unwrap()) as usize;
        p += 4;
        let mut v = Vec::with_capacity(dim);
        for _ in 0..dim {
            v.push(f32::from_le_bytes(b[p..p + 4].try_into().unwrap()));
            p += 4;
        }
        out.push(v);
    }
    out
}

fn write_attr(path: &Path, attr: &[u32]) {
    let mut buf = Vec::with_capacity(attr.len() * 4);
    for &a in attr {
        buf.extend_from_slice(&a.to_le_bytes());
    }
    fs::write(path, buf).unwrap();
}

fn read_attr(path: &Path) -> Vec<u32> {
    let mut b = Vec::new();
    fs::File::open(path).unwrap().read_to_end(&mut b).unwrap();
    b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Build one `.vss` segment over the rows `ids` (global ids), returning a resident
/// `FileSource` and the segment's measured recall curve.
fn build_segment(
    dir: &Path,
    idx: usize,
    ids: &[usize],
    base: &[Vec<f32>],
    attr: &[u32],
    dim: usize,
    m: usize,
) -> (FileSource, Vec<(usize, f64)>) {
    let mut hnsw = Hnsw::new(dim, HnswParams { m, ..HnswParams::default() });
    for &i in ids {
        hnsw.insert(base[i].clone());
    }
    hnsw.quantize();
    let offs: Vec<u32> = (0..ids.len() as u32).collect();
    let hnsw_bytes = serialize_hnsw(&hnsw, VEC, &offs);
    let curve = hnsw.measure_recall_curve(10, RECALL_EF_LADDER, 64);

    let columns = vec![
        Column {
            column_id: VEC,
            is_system: false,
            values: ColumnValues::Vector { dim: dim as u16, data: ids.iter().map(|&i| Some(base[i].clone())).collect() },
        },
        Column { column_id: CAT, is_system: false, values: ColumnValues::I64(ids.iter().map(|&i| Some(attr[i] as i64)).collect()) },
        Column { column_id: SYS_XMIN_COL, is_system: true, values: ColumnValues::I64(vec![Some(1); ids.len()]) },
    ];
    let meta = FileMeta {
        file_uuid: [0; 16], min_lsn: 1, max_lsn: 1, row_count: ids.len() as u64, schema_fingerprint: 0,
        creation_unix_nanos: 0, schema_blob: Vec::new(),
        mvcc: MvccSummary { min_xmin: 1, max_xmax: u64::MAX, tombstone_count: 0, live_count: ids.len() as u64 },
        writer_version: "ll-bench".into(),
    };
    let extra = [
        RawSection { kind: SectionType::Hnsw, column_id: VEC, encoding: SectionEncoding::Plain, bytes: hnsw_bytes },
        RawSection { kind: SectionType::OptimizerStats, column_id: VEC, encoding: SectionEncoding::Plain, bytes: encode_recall_curve(&curve) },
    ];
    let vss = dir.join(format!("seg-{idx}.vss"));
    let tt: Vec<u64> = ids.iter().map(|&i| i as u64).collect();
    write_file_with(&vss, &meta, &columns, &tt, &extra, &WriteOptions::default()).unwrap();
    let file = read_file(&vss).unwrap();
    (FileSource::new(std::sync::Arc::new(file)), curve)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "benchdata".into()));
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let dim: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(32);
    let q: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(50);
    let m: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(16);
    // 6th arg: "corr" makes the filter column correlate with vector position (adversarial:
    // the matching rows form a spatial slab, the hard case for filtered ANN). 7th arg: number
    // of .vss segments to split the data across (exercises cross-segment merge).
    let corr: bool = args.get(6).map(|s| s == "corr" || s == "1").unwrap_or(false);
    let segs: usize = args.get(7).and_then(|s| s.parse().ok()).filter(|&s| s >= 1).unwrap_or(1);
    fs::create_dir_all(&dir).unwrap();

    let base_path = dir.join("base.fvecs");
    if !base_path.exists() {
        eprintln!("generating dataset: {n} x {dim}, {q} queries, corr={corr} → {}", dir.display());
        let mut rng = Rng::new(42);
        let base: Vec<Vec<f32>> = (0..n).map(|_| (0..dim).map(|_| rng.f()).collect()).collect();
        let queries: Vec<Vec<f32>> = (0..q).map(|_| (0..dim).map(|_| rng.f()).collect()).collect();
        let attr: Vec<u32> = if corr {
            // attr = percentile rank of the vector's projection onto a fixed random direction,
            // so `attr < thr` selects a contiguous half-space slab — spatially correlated with
            // the vectors (a random query's true matches may lie far from its ANN candidates).
            let dir_vec: Vec<f32> = (0..dim).map(|_| rng.f() - 0.5).collect();
            let mut proj: Vec<(f32, usize)> = base
                .iter()
                .enumerate()
                .map(|(i, v)| (v.iter().zip(&dir_vec).map(|(a, b)| a * b).sum::<f32>(), i))
                .collect();
            proj.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut a = vec![0u32; n];
            for (rank, &(_, i)) in proj.iter().enumerate() {
                a[i] = ((rank * 1000) / n.max(1)) as u32;
            }
            a
        } else {
            (0..n).map(|_| (rng.next() % 1000) as u32).collect()
        };
        write_fvecs(&base_path, &base);
        write_fvecs(&dir.join("query.fvecs"), &queries);
        write_attr(&dir.join("attr.u32"), &attr);
    }

    let base = read_fvecs(&base_path);
    let queries = read_fvecs(&dir.join("query.fvecs"));
    let attr = read_attr(&dir.join("attr.u32"));
    let n = base.len();
    let dim = base.first().map(|v| v.len()).unwrap_or(0);
    eprintln!("LL: {n} x {dim} base, {} queries; building HNSW + .vss ...", queries.len());

    // Build the LL index/file(s), split across `segs` segments by global row id.
    let t = Instant::now();
    let chunk = n.div_ceil(segs);
    let mut file_sources: Vec<FileSource> = Vec::new();
    let mut recall_curve = Vec::new();
    for s in 0..segs {
        let ids: Vec<usize> = (s * chunk..((s + 1) * chunk).min(n)).collect();
        if ids.is_empty() {
            break;
        }
        let (fs, curve) = build_segment(&dir, s, &ids, &base, &attr, dim, m);
        if s == 0 {
            recall_curve = curve;
        }
        file_sources.push(fs);
    }
    let build_s = t.elapsed().as_secs_f64();
    let sources_vec: Vec<&dyn Source> = file_sources.iter().map(|f| f as &dyn Source).collect();
    let sources: &[&dyn Source] = &sources_vec;

    let curve_str: Vec<String> = recall_curve.iter().map(|(ef, r)| format!("ef{ef}={r:.3}")).collect();
    let seg_note = if segs > 1 { format!(" across {segs} segments (seg-0 curve)") } else { String::new() };
    println!("\nm={m}  corr={corr}  measured unfiltered recall@10 curve{seg_note}: {}", curve_str.join("  "));
    println!("(planner picks the smallest ef >= 0.9 target, else falls back to exact pre-filter)");
    println!("\n{:>6} {:>8}  {:<10} {:>8} {:>9}", "sel", "matched", "strategy", "recall", "ms/query");
    println!("{}", "-".repeat(48));

    let mut rows_json = Vec::new();
    for &sel in &SELS {
        let thr = (sel * 1000.0) as i64;
        let matched = attr.iter().filter(|&&a| (a as i64) < thr).count();
        let strategy = if explain_plan(sources, &Query::new(K).with_vector(VEC, queries[0].clone()).filter(CAT, PredOp::Lt, Value::I64(thr)), SNAP).contains("PreFilter") {
            "PreFilter"
        } else {
            "PostFilter"
        };

        let mut recall = 0.0;
        let mut nanos = 0u128;
        let mut counted = 0;
        for qv in &queries {
            let mut truth: Vec<(f32, u64)> = (0..n)
                .filter(|&i| (attr[i] as i64) < thr)
                .map(|i| (l2(qv, &base[i]), i as u64))
                .collect();
            truth.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let set: std::collections::HashSet<u64> = truth.into_iter().take(K).map(|(_, i)| i).collect();
            if set.is_empty() {
                continue;
            }
            counted += 1;
            let query = Query::new(K).with_vector(VEC, qv.clone()).filter(CAT, PredOp::Lt, Value::I64(thr));
            let start = Instant::now();
            let res = execute(sources, &query, SNAP);
            nanos += start.elapsed().as_nanos();
            let hits = res.iter().filter(|(id, _)| set.contains(id)).count();
            recall += hits as f64 / set.len() as f64;
        }
        let denom = counted.max(1) as f64;
        let r = recall / denom;
        let ms = nanos as f64 / denom / 1e6;
        println!("{sel:>6.3} {matched:>8} {strategy:<10} {r:>8.3} {ms:>9.3}");
        rows_json.push(format!(
            "    {{\"selectivity\": {sel}, \"matched\": {matched}, \"strategy\": \"{strategy}\", \"recall\": {r:.4}, \"latency_ms\": {ms:.4}}}"
        ));
    }

    let json = format!(
        "{{\n  \"system\": \"ll\",\n  \"dataset\": \"{n}x{dim}\",\n  \"k\": {K},\n  \"build_seconds\": {build_s:.2},\n  \"results\": [\n{}\n  ]\n}}\n",
        rows_json.join(",\n")
    );
    // ---- concurrent-query throughput (cross-query parallelism) ----
    // The same read-only sources are queried from many threads at once (Source: Sync).
    let thr = 100i64; // ~10% selectivity
    let workload: Vec<&Vec<f32>> = (0..2000).map(|i| &queries[i % queries.len()]).collect();
    let run = |qs: &[&Vec<f32>]| {
        for qv in qs {
            let q = Query::new(K).with_vector(VEC, (*qv).clone()).filter(CAT, PredOp::Lt, Value::I64(thr));
            std::hint::black_box(execute(sources, &q, SNAP));
        }
    };
    let t = Instant::now();
    run(&workload);
    let qps1 = workload.len() as f64 / t.elapsed().as_secs_f64();

    let nthreads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let t = Instant::now();
    let chunk = workload.len().div_ceil(nthreads);
    std::thread::scope(|s| {
        for ck in workload.chunks(chunk) {
            s.spawn(move || run(ck));
        }
    });
    let qps_n = workload.len() as f64 / t.elapsed().as_secs_f64();
    println!(
        "\nconcurrent throughput (sel~0.1): 1 thread {qps1:.0} q/s  |  {nthreads} threads {qps_n:.0} q/s  ({:.1}x)",
        qps_n / qps1
    );

    let results_dir = dir.join("results");
    fs::create_dir_all(&results_dir).unwrap();
    let out = results_dir.join("ll.json");
    fs::File::create(&out).unwrap().write_all(json.as_bytes()).unwrap();
    eprintln!("\nwrote {}", out.display());
}
