//! Filtered-vector-search benchmark: sweep predicate selectivity and compare the three
//! strategies (pre-filter / post-filter / integrated-filter) on recall@k and distance
//! evaluations (a hardware-independent cost proxy) plus wall-clock time.
//!
//! Run: `cargo run --release -p ll-index --example filtered_bench`
//!
//! The point is the crossover: no single strategy is cheapest everywhere, which is why a
//! cost-based optimizer must choose among them per query.

use std::time::Instant;

use ll_index::{
    distance, integrated, postfilter, prefilter, serialize_hnsw, Hnsw, HnswParams, HnswView,
    Metric, SplitMix64,
};

const N: usize = 5000;
const DIM: usize = 32;
const K: usize = 10;
const EF: usize = 64;
const QUERIES: usize = 50;
const METRIC: Metric = Metric::L2;

fn gen_vectors(seed: u64, n: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| (0..dim).map(|_| rng.next_f64() as f32).collect())
        .collect()
}

/// Ground truth: exact top-k among rows passing the predicate.
fn filtered_truth(
    query: &[f32],
    vectors: &[Vec<f32>],
    predicate: &dyn Fn(u32) -> bool,
    k: usize,
) -> Vec<u32> {
    let mut scored: Vec<(f32, u32)> = vectors
        .iter()
        .enumerate()
        .filter(|(i, _)| predicate(*i as u32))
        .map(|(i, v)| (distance(METRIC, query, v), i as u32))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(k).map(|(_, i)| i).collect()
}

fn recall(got: &[(u32, f32)], truth: &[u32]) -> f64 {
    if truth.is_empty() {
        return 1.0;
    }
    let set: std::collections::HashSet<u32> = truth.iter().copied().collect();
    let hits = got.iter().filter(|(id, _)| set.contains(id)).count();
    hits as f64 / truth.len() as f64
}

fn main() {
    eprintln!("building HNSW: {N} vectors x {DIM}d ...");
    let data = gen_vectors(1, N, DIM);
    let mut index = Hnsw::new(
        DIM,
        HnswParams { m: 16, ef_construction: 100, seed: 42, metric: METRIC },
    );
    for v in &data {
        index.insert(v.clone());
    }
    index.quantize();
    let local_offsets: Vec<u32> = (0..N as u32).collect();
    let bytes = serialize_hnsw(&index, 1, &local_offsets);
    let view = HnswView::parse(&bytes).expect("parse");
    eprintln!("serialized HNSW section: {} bytes\n", bytes.len());

    // Each row gets a uniform filter value in [0,1); predicate(s) = value < s ⇒ selectivity ≈ s.
    let filt = gen_vectors(7, N, 1);
    let value = |id: u32| filt[id as usize][0];

    let queries = gen_vectors(999, QUERIES, DIM);
    let selectivities = [0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.25, 0.5, 1.0];

    println!(
        "{:>6}  {:<11} {:>8} {:>10} {:>9}   optimal (recall>=0.90, min evals)",
        "sel", "strategy", "recall", "dist_eval", "us/query"
    );
    println!("{}", "-".repeat(86));

    for &s in &selectivities {
        let predicate = |id: u32| value(id) < s as f32;
        let matching = (0..N as u32).filter(|&i| predicate(i)).count();

        let mut rows: Vec<(&str, f64, f64, f64)> = Vec::new(); // name, recall, evals, us
        for name in ["prefilter", "postfilter", "integrated"] {
            let mut tot_recall = 0.0;
            let mut tot_evals = 0usize;
            let mut tot_nanos = 0u128;
            for q in &queries {
                let truth = filtered_truth(q, &data, &predicate, K);
                let start = Instant::now();
                let res = match name {
                    "prefilter" => prefilter(q, &data, predicate, K, METRIC),
                    "postfilter" => postfilter(&view, q, &data, predicate, K, EF, METRIC),
                    _ => integrated(&view, q, &data, predicate, K, EF, METRIC),
                };
                tot_nanos += start.elapsed().as_nanos();
                tot_recall += recall(&res.results, &truth);
                tot_evals += res.distance_evals;
            }
            let n = queries.len() as f64;
            rows.push((
                name,
                tot_recall / n,
                tot_evals as f64 / n,
                tot_nanos as f64 / n / 1000.0,
            ));
        }

        // Cost-optimal among strategies meeting the recall bar.
        let optimal = rows
            .iter()
            .filter(|(_, r, _, _)| *r >= 0.90)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(n, _, _, _)| *n)
            .unwrap_or("(none>=0.90)");

        for (i, (name, r, e, us)) in rows.iter().enumerate() {
            let tag = if i == 0 {
                format!("{s:>6.3}")
            } else {
                " ".repeat(6)
            };
            let opt = if i == 0 {
                format!("{optimal}  (~{matching} match)")
            } else {
                String::new()
            };
            println!("{tag}  {name:<11} {r:>8.3} {e:>10.0} {us:>9.1}   {opt}");
        }
        println!();
    }
}
