//! The three filtered-vector-search strategies, instrumented with a distance-evaluation
//! count so their cost can be compared honestly (the cost model's currency, hardware-
//! independent). This is the heart of the LL thesis: which strategy wins depends on the
//! predicate's selectivity, so a cost-based optimizer must choose among them.
//!
//! - **pre-filter**: evaluate the predicate, brute-force exact distance over the matching
//!   rows. Exact (recall 1.0). Cost ∝ number of matching rows.
//! - **post-filter**: run unfiltered HNSW, keep the candidates that pass the predicate,
//!   rerank. Cheap when the predicate is loose; misses results when it is tight.
//! - **integrated-filter**: HNSW traversal that only admits predicate-passing rows to the
//!   result set while still exploring through the rest. Robust across selectivities; pays
//!   extra traversal when the predicate is tight.
//!
//! A row is identified by its `local_offset`; `vectors[local_offset]` is its full-
//! precision vector (here held in memory; in the engine it comes from the column chunk).

use crate::hnsw::{distance, Metric};
use crate::page::HnswView;

/// Top-k results (`(local_offset, exact_distance)`) plus the distance-evaluation count.
#[derive(Debug, Clone)]
pub struct FilteredResult {
    pub results: Vec<(u32, f32)>,
    pub distance_evals: usize,
}

fn top_k(mut scored: Vec<(f32, u32)>, k: usize) -> Vec<(u32, f32)> {
    scored.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.truncate(k);
    scored.into_iter().map(|(d, i)| (i, d)).collect()
}

/// Pre-filter: exact brute force over rows passing `predicate`. Always exact.
pub fn prefilter(
    query: &[f32],
    vectors: &[Vec<f32>],
    predicate: impl Fn(u32) -> bool,
    k: usize,
    metric: Metric,
) -> FilteredResult {
    let mut evals = 0;
    let mut scored = Vec::new();
    for (i, v) in vectors.iter().enumerate() {
        if predicate(i as u32) {
            scored.push((distance(metric, query, v), i as u32));
            evals += 1;
        }
    }
    FilteredResult {
        results: top_k(scored, k),
        distance_evals: evals,
    }
}

/// Post-filter: unfiltered HNSW candidates, then drop those failing `predicate`, rerank.
pub fn postfilter(
    view: &HnswView,
    query: &[f32],
    vectors: &[Vec<f32>],
    predicate: impl Fn(u32) -> bool,
    k: usize,
    ef: usize,
    metric: Metric,
) -> FilteredResult {
    let (cands, mut evals) = view.search_counted(query, ef);
    let mut scored = Vec::new();
    for nb in cands {
        if predicate(nb.local_offset) {
            scored.push((
                distance(metric, query, &vectors[nb.local_offset as usize]),
                nb.local_offset,
            ));
            evals += 1;
        }
    }
    FilteredResult {
        results: top_k(scored, k),
        distance_evals: evals,
    }
}

/// Integrated-filter: HNSW traversal that only admits predicate-passing rows, then rerank.
pub fn integrated(
    view: &HnswView,
    query: &[f32],
    vectors: &[Vec<f32>],
    predicate: impl Fn(u32) -> bool,
    k: usize,
    ef: usize,
    metric: Metric,
) -> FilteredResult {
    let (cands, mut evals) = view.search_filtered(query, ef, &predicate);
    let mut scored = Vec::new();
    for nb in cands {
        scored.push((
            distance(metric, query, &vectors[nb.local_offset as usize]),
            nb.local_offset,
        ));
        evals += 1;
    }
    FilteredResult {
        results: top_k(scored, k),
        distance_evals: evals,
    }
}
