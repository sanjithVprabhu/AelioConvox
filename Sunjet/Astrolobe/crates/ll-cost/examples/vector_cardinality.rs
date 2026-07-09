//! Accuracy of centroid-distance vector cardinality estimation vs ground truth.
//!
//! Run: `cargo run --release -p ll-cost --example vector_cardinality`
//!
//! For each query we pick a distance threshold at a target true-selectivity quantile, then
//! compare the estimator's prediction to the (known) true selectivity. Reports mean
//! absolute error per target selectivity.

use ll_cost::VectorStats;
use ll_index::SplitMix64;

const N: usize = 5000;
const DIM: usize = 32;
const K: usize = 64;
const BUCKETS: usize = 64;
const QUERIES: usize = 50;

fn gen(seed: u64, n: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| (0..dim).map(|_| rng.next_f64() as f32).collect())
        .collect()
}

fn l2_sq(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = (*x - *y) as f64;
            d * d
        })
        .sum()
}

fn main() {
    let data = gen(1, N, DIM);
    let stats = VectorStats::build(&data, K, BUCKETS);
    let queries = gen(999, QUERIES, DIM);
    eprintln!("VectorStats: {} centroids over {N} vectors x {DIM}d\n", stats.num_centroids());

    println!("{:>10}  {:>9}  {:>9}", "target_sel", "mean_est", "MAE");
    println!("{}", "-".repeat(34));
    for &p in &[0.01f64, 0.05, 0.1, 0.25, 0.5] {
        let mut sum_est = 0.0;
        let mut sum_abs_err = 0.0;
        for q in &queries {
            let mut dists: Vec<f64> = data.iter().map(|v| l2_sq(q, v)).collect();
            dists.sort_by(|a, b| a.total_cmp(b));
            let idx = ((p * N as f64) as usize).clamp(1, N - 1);
            let threshold = dists[idx];
            let true_sel = idx as f64 / N as f64;
            let est = stats.estimate_selectivity_within(q, threshold);
            sum_est += est;
            sum_abs_err += (est - true_sel).abs();
        }
        let nq = queries.len() as f64;
        println!("{p:>10.3}  {:>9.3}  {:>9.3}", sum_est / nq, sum_abs_err / nq);
    }
}
