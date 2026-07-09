//! Validates that centroid-distance vector cardinality estimates are accurate at high
//! selectivity and discriminative across the selectivity range (the property the cost
//! model relies on to compare anchors). See `examples/vector_cardinality.rs` for the full
//! accuracy sweep.

use ll_cost::VectorStats;
use ll_index::SplitMix64;

const N: usize = 2000;
const DIM: usize = 32;

fn gen(seed: u64, n: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| (0..dim).map(|_| rng.next_f64() as f32).collect())
        .collect()
}

fn l2_sq(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| ((*x - *y) as f64).powi(2)).sum()
}

/// (mean estimate, mean absolute error) at a target true selectivity `p`.
fn measure(stats: &VectorStats, data: &[Vec<f32>], queries: &[Vec<f32>], p: f64) -> (f64, f64) {
    let mut sum_est = 0.0;
    let mut sum_err = 0.0;
    for q in queries {
        let mut dists: Vec<f64> = data.iter().map(|v| l2_sq(q, v)).collect();
        dists.sort_by(|a, b| a.total_cmp(b));
        let idx = ((p * N as f64) as usize).clamp(1, N - 1);
        let threshold = dists[idx];
        let true_sel = idx as f64 / N as f64;
        let est = stats.estimate_selectivity_within(q, threshold);
        sum_est += est;
        sum_err += (est - true_sel).abs();
    }
    let nq = queries.len() as f64;
    (sum_est / nq, sum_err / nq)
}

#[test]
fn estimates_are_accurate_and_discriminative() {
    let data = gen(1, N, DIM);
    let stats = VectorStats::build(&data, 64, 64);
    let queries = gen(999, 20, DIM);

    let (est_lo, mae_lo) = measure(&stats, &data, &queries, 0.01);
    let (est_mid, _) = measure(&stats, &data, &queries, 0.1);
    let (est_hi, _) = measure(&stats, &data, &queries, 0.5);

    // Accurate where it matters most (high selectivity = few matches).
    assert!(mae_lo < 0.10, "MAE at selectivity 0.01 was {mae_lo:.3}");

    // Discriminative: the estimate must separate selective from non-selective predicates.
    assert!(est_lo < est_mid && est_mid < est_hi, "estimates not monotone: {est_lo:.3} {est_mid:.3} {est_hi:.3}");
    assert!(est_lo < 0.2, "selective predicate over-estimated: {est_lo:.3}");
    assert!(est_hi > 0.4, "non-selective predicate under-estimated: {est_hi:.3}");
}
