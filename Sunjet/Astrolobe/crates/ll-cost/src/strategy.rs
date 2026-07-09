//! Filtered-vector strategy selection from estimated selectivity.
//!
//! Cost is in *distance evaluations* — the same currency the benchmark measured, and a
//! hardware-independent proxy for the spec's microsecond cost model. The selector predicts
//! each strategy's cost and expected recall, then picks the cheapest strategy expected to
//! meet the recall target. The crossover it produces matches the empirical benchmark.

/// The filtered-vector-search strategies (mirrors `ll-index`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    PreFilter,
    PostFilter,
    Integrated,
}

/// Inputs to the vector-search cost model. `hnsw_base_evals` is the measured cost of an
/// unfiltered HNSW search at this `ef` (calibrated against the real index). `base_ann_recall`
/// is the index's measured unfiltered recall@k (1.0 = exact, e.g. a brute-force memtable);
/// it caps what post-filter can achieve and falls with dimensionality.
#[derive(Debug, Clone, Copy)]
pub struct VecCostParams {
    pub n: usize,
    pub k: usize,
    pub ef: usize,
    pub hnsw_base_evals: f64,
    pub base_ann_recall: f64,
}

/// A predicted cost + recall for one strategy.
#[derive(Debug, Clone, Copy)]
pub struct CostEstimate {
    pub strategy: Strategy,
    pub cost_evals: f64,
    pub expected_recall: f64,
}

/// Predict cost + recall for all three strategies at the given predicate `selectivity`.
///
/// - **pre-filter**: exact; cost ∝ matching rows (`selectivity * n`). Recall 1.0 regardless
///   of dimensionality — it computes true distances over the matching rows.
/// - **post-filter**: ~constant HNSW base cost (the survivor rerank is negligible). Its
///   recall score, `base_ann_recall * min(1, ef*selectivity/k)`, combines two caps: the
///   index's unfiltered ANN recall (which falls with dimensionality), and a `min(1, …)`
///   selectivity term. The latter is a *decision proxy* — at low selectivity post-filter must
///   over-fetch heavily, making it both slower and a poor choice, so the score steers the
///   planner to pre-filter there. When the ANN cap alone dips below the target, post-filter is
///   excluded at every selectivity and the planner falls back to exact pre-filter.
/// - **integrated**: cost grows as selectivity falls (it must explore ~`base/selectivity`),
///   capped at a full scan `n`; recall also bounded by `base_ann_recall`. Modeled for
///   analysis but never selected by [`choose`].
pub fn estimate_all(selectivity: f64, p: &VecCostParams) -> [CostEstimate; 3] {
    let sel = selectivity.clamp(0.0, 1.0);
    let n = p.n as f64;
    let ef = p.ef as f64;
    let k = p.k.max(1) as f64;
    let ann = p.base_ann_recall.clamp(0.0, 1.0);

    let pre = CostEstimate {
        strategy: Strategy::PreFilter,
        cost_evals: sel * n,
        expected_recall: 1.0,
    };
    let post = CostEstimate {
        strategy: Strategy::PostFilter,
        cost_evals: p.hnsw_base_evals,
        expected_recall: ann * (ef * sel / k).min(1.0),
    };
    let integrated = CostEstimate {
        strategy: Strategy::Integrated,
        cost_evals: (p.hnsw_base_evals / sel.max(1e-9)).min(n),
        expected_recall: ann,
    };
    [pre, post, integrated]
}

/// Choose the cheapest strategy expected to meet `recall_target`. Pre-filter is always a
/// safe fallback (exact), so a choice always exists.
///
/// Only the two strategies the engine actually executes — pre-filter and post-filter — are
/// candidates. `Integrated` is modeled in [`estimate_all`] (for `explain`/analysis) but
/// never selected: empirically it never beat both pre- and post-filter on the recall-safe
/// frontier, and its cost estimate is optimistic, so returning it would only mislead the
/// executor (which would run pre-filter for it anyway).
pub fn choose(selectivity: f64, p: &VecCostParams, recall_target: f64) -> Strategy {
    estimate_all(selectivity, p)
        .into_iter()
        .filter(|e| e.strategy != Strategy::Integrated && e.expected_recall >= recall_target)
        .min_by(|a, b| a.cost_evals.total_cmp(&b.cost_evals))
        .map(|e| e.strategy)
        .unwrap_or(Strategy::PreFilter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> VecCostParams {
        VecCostParams {
            n: 5000,
            k: 10,
            ef: 64,
            hnsw_base_evals: 1400.0,
            base_ann_recall: 1.0,
        }
    }

    #[test]
    fn low_selectivity_picks_prefilter() {
        let p = params();
        for sel in [0.002, 0.01, 0.05, 0.1, 0.2] {
            assert_eq!(choose(sel, &p, 0.9), Strategy::PreFilter, "sel {sel}");
        }
    }

    #[test]
    fn high_selectivity_picks_postfilter() {
        let p = params();
        for sel in [0.4, 0.5, 0.75, 1.0] {
            assert_eq!(choose(sel, &p, 0.9), Strategy::PostFilter, "sel {sel}");
        }
    }

    #[test]
    fn crossover_near_base_over_n() {
        // Pre-filter wins while selectivity*n < hnsw cost (~base/n); post-filter wins
        // above. With base=1400, n=5000 the crossover is ~0.28.
        let p = params();
        assert_eq!(choose(0.25, &p, 0.9), Strategy::PreFilter);
        assert_eq!(choose(0.32, &p, 0.9), Strategy::PostFilter);
    }
}
