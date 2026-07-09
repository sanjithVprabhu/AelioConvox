//! Locks the filtered-vector-search thesis: no single strategy wins across selectivities,
//! so a cost-based optimizer must choose. See `examples/filtered_bench.rs` for the full
//! sweep; this test asserts the robust qualitative findings.

use ll_index::{
    distance, integrated, postfilter, prefilter, serialize_hnsw, Hnsw, HnswParams, HnswView,
    Metric, SplitMix64,
};

const N: usize = 2000;
const DIM: usize = 24;
const K: usize = 10;
const EF: usize = 64;
const METRIC: Metric = Metric::L2;

fn gen(seed: u64, n: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| (0..dim).map(|_| rng.next_f64() as f32).collect())
        .collect()
}

fn truth(query: &[f32], vectors: &[Vec<f32>], pred: &dyn Fn(u32) -> bool, k: usize) -> Vec<u32> {
    let mut s: Vec<(f32, u32)> = vectors
        .iter()
        .enumerate()
        .filter(|(i, _)| pred(*i as u32))
        .map(|(i, v)| (distance(METRIC, query, v), i as u32))
        .collect();
    s.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    s.into_iter().take(k).map(|(_, i)| i).collect()
}

fn recall(got: &[(u32, f32)], truth: &[u32]) -> f64 {
    if truth.is_empty() {
        return 1.0;
    }
    let set: std::collections::HashSet<u32> = truth.iter().copied().collect();
    got.iter().filter(|(id, _)| set.contains(id)).count() as f64 / truth.len() as f64
}

struct Harness {
    data: Vec<Vec<f32>>,
    view_bytes: Vec<u8>,
    queries: Vec<Vec<f32>>,
    filt: Vec<f32>,
}

impl Harness {
    fn build() -> Self {
        let data = gen(1, N, DIM);
        let mut index = Hnsw::new(
            DIM,
            HnswParams { m: 16, ef_construction: 100, seed: 42, metric: METRIC },
        );
        for v in &data {
            index.insert(v.clone());
        }
        index.quantize();
        let local_offsets: Vec<u32> = (0..N as u32).collect();
        let view_bytes = serialize_hnsw(&index, 1, &local_offsets);
        let queries = gen(999, 20, DIM);
        let filt: Vec<f32> = gen(7, N, 1).into_iter().map(|v| v[0]).collect();
        Harness { data, view_bytes, queries, filt }
    }

    /// Average (recall, distance_evals) for a strategy at selectivity `s`.
    fn run(&self, strategy: &str, s: f32) -> (f64, f64) {
        let view = HnswView::parse(&self.view_bytes).unwrap();
        let pred = |id: u32| self.filt[id as usize] < s;
        let mut tot_r = 0.0;
        let mut tot_e = 0usize;
        for q in &self.queries {
            let t = truth(q, &self.data, &pred, K);
            let res = match strategy {
                "prefilter" => prefilter(q, &self.data, pred, K, METRIC),
                "postfilter" => postfilter(&view, q, &self.data, pred, K, EF, METRIC),
                _ => integrated(&view, q, &self.data, pred, K, EF, METRIC),
            };
            tot_r += recall(&res.results, &t);
            tot_e += res.distance_evals;
        }
        let n = self.queries.len() as f64;
        (tot_r / n, tot_e as f64 / n)
    }
}

#[test]
fn prefilter_is_always_exact() {
    let h = Harness::build();
    for s in [0.01f32, 0.1, 0.5, 1.0] {
        let (r, _) = h.run("prefilter", s);
        assert!(r > 0.999, "prefilter recall at s={s} was {r:.3}");
    }
}

#[test]
fn postfilter_collapses_but_integrated_is_robust_at_low_selectivity() {
    // The core failure mode: at low selectivity, naive post-filtering returns mostly
    // non-matching candidates (low recall); integrated-filter stays accurate.
    let h = Harness::build();
    let (post_r, _) = h.run("postfilter", 0.01);
    let (int_r, _) = h.run("integrated", 0.01);
    assert!(post_r < 0.5, "expected post-filter to collapse, got {post_r:.3}");
    assert!(int_r >= 0.95, "expected integrated to stay robust, got {int_r:.3}");
}

#[test]
fn cost_optimal_strategy_depends_on_selectivity() {
    // The thesis: prefilter is cheapest when few rows match; an HNSW-based strategy is
    // cheaper when most rows match. Hence the optimizer must choose per query.
    let h = Harness::build();

    // Low selectivity: prefilter does far fewer distance evals than integrated.
    let (_, pre_lo) = h.run("prefilter", 0.01);
    let (_, int_lo) = h.run("integrated", 0.01);
    assert!(
        pre_lo < int_lo,
        "at low selectivity prefilter ({pre_lo:.0}) should be cheaper than integrated ({int_lo:.0})"
    );

    // Full selectivity (no filter): prefilter scans everything (== N); post-filter (HNSW)
    // is much cheaper while keeping high recall.
    let (post_r_hi, post_hi) = h.run("postfilter", 1.0);
    let (_, pre_hi) = h.run("prefilter", 1.0);
    assert!((pre_hi - N as f64).abs() < 1.0, "prefilter at s=1.0 should scan all N");
    assert!(
        post_hi < pre_hi,
        "at full selectivity HNSW post-filter ({post_hi:.0}) should beat prefilter ({pre_hi:.0})"
    );
    assert!(post_r_hi >= 0.9, "post-filter recall at s=1.0 was {post_r_hi:.3}");
}
