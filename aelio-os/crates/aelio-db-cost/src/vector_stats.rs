//! Per-file vector cardinality estimation via centroid distance distributions.
//!
//! The genuinely novel piece of the cost model (spec §Part X): estimate how many vectors
//! fall within distance `T` of a query vector — needed to plan vector-range queries and to
//! pick the most selective anchor in a hybrid query. The distance distribution depends on
//! where the query sits in the embedding space, so a single global histogram won't do.
//!
//! Approach: sample `k` representative vectors as centroids; for each, store the
//! distribution (an equi-height [`Histogram`]) of distances from that centroid to *all*
//! vectors. At query time, find the nearest centroid and read its distribution. The closer
//! the query is to a sampled centroid, the better the estimate. Distances are squared-L2
//! (monotonic with L2, matching `aelio-db-index`'s `Metric::L2`).

use crate::histogram::Histogram;

/// Squared-L2 distance.
fn l2_sq(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = (*x - *y) as f64;
            d * d
        })
        .sum()
}

/// Centroid distance distributions for one file's vector column.
#[derive(Debug, Clone)]
pub struct VectorStats {
    centroids: Vec<Vec<f32>>,
    /// `histograms[i]` = distribution of squared-L2 distances from `centroids[i]` to all
    /// vectors in the file.
    histograms: Vec<Histogram>,
}

impl VectorStats {
    /// Build from a file's vectors. Centroids are *sampled* data points (even striding) —
    /// statistically query-like, since queries are drawn from the same distribution as the
    /// data. (Cluster *means* are a poor choice here: in high dimensions they regress to
    /// the cloud's center while real queries live at the periphery, badly over-estimating.)
    /// Each centroid gets a `buckets`-bucket histogram of its squared-L2 distances to *all*
    /// vectors. Cost: O(k · n · dim), once at compaction. Deterministic.
    pub fn build(vectors: &[Vec<f32>], num_centroids: usize, buckets: usize) -> Self {
        let n = vectors.len();
        if n == 0 {
            return VectorStats {
                centroids: Vec::new(),
                histograms: Vec::new(),
            };
        }
        let k = num_centroids.clamp(1, n);
        let centroids: Vec<Vec<f32>> = (0..k).map(|i| vectors[i * n / k].clone()).collect();
        let histograms = centroids
            .iter()
            .map(|c| {
                let dists: Vec<f64> = vectors.iter().map(|v| l2_sq(c, v)).collect();
                Histogram::build(&dists, buckets)
            })
            .collect();
        VectorStats {
            centroids,
            histograms,
        }
    }

    pub fn num_centroids(&self) -> usize {
        self.centroids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.centroids.is_empty()
    }

    /// Estimate the fraction of vectors within squared-L2 distance `threshold` of `query`,
    /// using the nearest centroid's distance distribution as the proxy.
    pub fn estimate_selectivity_within(&self, query: &[f32], threshold: f64) -> f64 {
        if self.histograms.is_empty() {
            return 0.0;
        }
        let mut best = 0usize;
        let mut best_d = f64::INFINITY;
        for (i, c) in self.centroids.iter().enumerate() {
            let d = l2_sq(c, query);
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        self.histograms[best].selectivity_lt(threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_safe() {
        let s = VectorStats::build(&[], 8, 16);
        assert!(s.is_empty());
        assert_eq!(s.estimate_selectivity_within(&[0.0, 0.0], 1.0), 0.0);
    }

    #[test]
    fn query_at_centroid_estimates_well() {
        // A clustered 1-D-ish set; querying exactly at a centroid should give a near-exact
        // estimate from that centroid's own distribution.
        let vectors: Vec<Vec<f32>> = (0..1000).map(|i| vec![i as f32, 0.0]).collect();
        let stats = VectorStats::build(&vectors, 32, 64);
        let query = vec![500.0, 0.0];
        // threshold of 100^2 around x=500 covers x in (400,600) ≈ 200/1000 = 0.2
        let est = stats.estimate_selectivity_within(&query, 100.0 * 100.0);
        assert!((est - 0.2).abs() < 0.05, "est {est:.3}");
    }
}
