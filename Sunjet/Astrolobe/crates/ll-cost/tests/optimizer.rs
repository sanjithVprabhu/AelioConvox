//! End-to-end: estimate selectivity from a histogram, let the cost model choose a
//! filtered-vector strategy, and assert the choice matches the empirically cheapest
//! strategy (meeting the recall bar) on the real `ll-index` engine.

use ll_cost::{choose, Histogram, Strategy, VecCostParams};
use ll_index::{
    distance, integrated, postfilter, prefilter, serialize_hnsw, Hnsw, HnswParams, HnswView,
    Metric, SplitMix64,
};

const N: usize = 2000;
const DIM: usize = 24;
const K: usize = 10;
const EF: usize = 64;
const TARGET: f64 = 0.9;
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

#[test]
fn cost_model_picks_the_empirically_optimal_strategy() {
    // --- build + serialize the index ---
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
    let bytes = serialize_hnsw(&index, 1, &local_offsets);
    let view = HnswView::parse(&bytes).unwrap();

    let queries = gen(999, 20, DIM);
    let filt: Vec<f32> = gen(7, N, 1).into_iter().map(|v| v[0]).collect();

    // --- calibrate hnsw_base_evals from real unfiltered searches ---
    let base: f64 = {
        let total: usize = queries.iter().map(|q| view.search_counted(q, EF).1).sum();
        total as f64 / queries.len() as f64
    };
    // At DIM=24 the unfiltered ANN recall is ~1.0, so the cap doesn't bind here.
    let params = VecCostParams { n: N, k: K, ef: EF, hnsw_base_evals: base, base_ann_recall: 1.0 };

    // --- statistics: histogram of the filter column ---
    let hist = Histogram::build(&filt.iter().map(|&x| x as f64).collect::<Vec<_>>(), 64);

    // --- empirical optimal at a selectivity (min evals among recall >= TARGET) ---
    let empirical = |s: f32| -> Strategy {
        let pred = |id: u32| filt[id as usize] < s;
        let mut acc = [(0.0f64, 0usize); 3]; // (recall_sum, evals_sum) for pre/post/int
        for q in &queries {
            let t = truth(q, &data, &pred, K);
            let r0 = prefilter(q, &data, pred, K, METRIC);
            let r1 = postfilter(&view, q, &data, pred, K, EF, METRIC);
            let r2 = integrated(&view, q, &data, pred, K, EF, METRIC);
            for (i, r) in [&r0, &r1, &r2].into_iter().enumerate() {
                acc[i].0 += recall(&r.results, &t);
                acc[i].1 += r.distance_evals;
            }
        }
        let nq = queries.len() as f64;
        let strategies = [Strategy::PreFilter, Strategy::PostFilter, Strategy::Integrated];
        strategies
            .into_iter()
            .enumerate()
            .filter(|(i, _)| acc[*i].0 / nq >= TARGET)
            .min_by(|(i, _), (j, _)| acc[*i].1.cmp(&acc[*j].1))
            .map(|(_, s)| s)
            .unwrap_or(Strategy::PreFilter)
    };

    let sweep = [0.01f32, 0.05, 0.1, 0.25, 0.5, 1.0];
    eprintln!("base(hnsw)={base:.0}  n={N}  ef={EF}  k={K}");
    eprintln!("{:>6}  {:>8} {:>12} {:>12}", "sel", "est_sel", "model", "empirical");
    for &s in &sweep {
        let est = hist.selectivity_lt(s as f64);
        let modeled = choose(est, &params, TARGET);
        let emp = empirical(s);
        eprintln!("{s:>6.3}  {est:>8.3} {modeled:>12?} {emp:>12?}");

        // The histogram estimate should track the true (uniform) selectivity.
        assert!((est - s as f64).abs() < 0.04, "sel est {est:.3} vs {s}");
        // The model's choice should match what actually wins.
        assert_eq!(modeled, emp, "strategy mismatch at selectivity {s}");
    }
}
