//! Equi-height histogram for scalar selectivity estimation.
//!
//! Each of `buckets` buckets holds ~the same number of values, so bucket *mass* is
//! uniform (`1/buckets`) and boundaries cluster where data is dense — robust to skew, the
//! property the architecture spec asks for. Built from a column sample at compaction time;
//! here we build directly from values. Selectivity of a predicate is estimated by linear
//! interpolation within the bucket containing the threshold.

/// An equi-height histogram over `f64` values.
#[derive(Debug, Clone)]
pub struct Histogram {
    /// `buckets + 1` quantile boundaries, ascending.
    bounds: Vec<f64>,
    total: u64,
}

impl Histogram {
    /// Build an equi-height histogram with `buckets` buckets from `values`.
    pub fn build(values: &[f64], buckets: usize) -> Self {
        let buckets = buckets.max(1);
        let mut sorted: Vec<f64> = values.to_vec();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let n = sorted.len();
        if n == 0 {
            return Histogram {
                bounds: vec![0.0, 0.0],
                total: 0,
            };
        }
        let mut bounds = Vec::with_capacity(buckets + 1);
        for i in 0..=buckets {
            // quantile boundary at fraction i/buckets
            let idx = ((i as f64 / buckets as f64) * (n - 1) as f64).round() as usize;
            bounds.push(sorted[idx.min(n - 1)]);
        }
        Histogram {
            bounds,
            total: n as u64,
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    fn buckets(&self) -> usize {
        self.bounds.len() - 1
    }

    /// Estimated fraction of values strictly less than `threshold`, in `[0, 1]`.
    pub fn selectivity_lt(&self, threshold: f64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let b = self.buckets();
        let lo = self.bounds[0];
        let hi = self.bounds[b];
        if threshold <= lo {
            return 0.0;
        }
        if threshold >= hi {
            return 1.0;
        }
        // Find the bucket [bounds[j], bounds[j+1]) containing the threshold.
        for j in 0..b {
            let left = self.bounds[j];
            let right = self.bounds[j + 1];
            if threshold < right || j == b - 1 {
                let width = right - left;
                let frac = if width > 0.0 {
                    ((threshold - left) / width).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                return ((j as f64) + frac) / b as f64;
            }
        }
        1.0
    }

    /// Estimated fraction of values in `[lo, hi)`.
    pub fn selectivity_range(&self, lo: f64, hi: f64) -> f64 {
        (self.selectivity_lt(hi) - self.selectivity_lt(lo)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_selectivity_is_accurate() {
        let values: Vec<f64> = (0..10_000).map(|i| i as f64 / 10_000.0).collect();
        let h = Histogram::build(&values, 64);
        for t in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let est = h.selectivity_lt(t);
            assert!((est - t).abs() < 0.02, "lt({t}) est {est:.3}");
        }
        assert_eq!(h.selectivity_lt(-1.0), 0.0);
        assert_eq!(h.selectivity_lt(2.0), 1.0);
    }

    #[test]
    fn range_selectivity() {
        let values: Vec<f64> = (0..10_000).map(|i| i as f64 / 10_000.0).collect();
        let h = Histogram::build(&values, 64);
        assert!((h.selectivity_range(0.2, 0.5) - 0.3).abs() < 0.02);
    }

    #[test]
    fn empty_histogram_is_safe() {
        let h = Histogram::build(&[], 16);
        assert_eq!(h.selectivity_lt(0.5), 0.0);
    }
}
