//! Recall@k of HNSW search vs brute-force ground truth, plus a determinism check.

use aelio_db_index::{distance, Hnsw, HnswParams, Metric, SplitMix64};

fn gen_vectors(seed: u64, n: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| (0..dim).map(|_| rng.next_f64() as f32).collect())
        .collect()
}

fn brute_force(query: &[f32], vectors: &[Vec<f32>], k: usize, metric: Metric) -> Vec<u32> {
    let mut scored: Vec<(f32, u32)> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (distance(metric, query, v), i as u32))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(k).map(|(_, i)| i).collect()
}

#[test]
fn recall_at_10_is_high_on_random_data() {
    let dim = 24;
    let n = 1500;
    let k = 10;
    let ef_search = 100;
    let metric = Metric::L2;

    let data = gen_vectors(1, n, dim);
    let mut index = Hnsw::new(
        dim,
        HnswParams {
            m: 16,
            ef_construction: 200,
            seed: 42,
            metric,
        },
    );
    for v in &data {
        index.insert(v.clone());
    }
    assert_eq!(index.len(), n);

    let queries = gen_vectors(999, 40, dim);
    let mut total_recall = 0.0;
    for q in &queries {
        let truth: std::collections::HashSet<u32> =
            brute_force(q, &data, k, metric).into_iter().collect();
        let got = index.search(q, k, ef_search);
        let hits = got.iter().filter(|(id, _)| truth.contains(id)).count();
        total_recall += hits as f64 / k as f64;
    }
    let recall = total_recall / queries.len() as f64;
    eprintln!("recall@{k} (ef_search={ef_search}) = {recall:.4}");
    assert!(recall >= 0.90, "recall@{k} = {recall:.3}, expected >= 0.90");
}

#[test]
fn results_are_sorted_by_distance() {
    let data = gen_vectors(7, 300, 8);
    let mut index = Hnsw::new(8, HnswParams::default());
    for v in &data {
        index.insert(v.clone());
    }
    let res = index.search(&data[0], 10, 64);
    for w in res.windows(2) {
        assert!(w[0].1 <= w[1].1, "results must be ascending by distance");
    }
}

#[test]
fn build_is_deterministic_for_a_seed() {
    let data = gen_vectors(3, 500, 12);
    let params = HnswParams {
        m: 16,
        ef_construction: 100,
        seed: 123,
        metric: Metric::L2,
    };

    let build = || {
        let mut index = Hnsw::new(12, params);
        for v in &data {
            index.insert(v.clone());
        }
        index
    };
    let a = build();
    let b = build();

    let q = &data[42];
    assert_eq!(a.search(q, 10, 64), b.search(q, 10, 64));
}
