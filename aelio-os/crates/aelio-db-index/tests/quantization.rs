//! Recall of quantized-traversal + f32-rerank vs brute-force ground truth, and the
//! recall delta against full-precision HNSW (target: <2% loss).

use aelio_db_index::{distance, Hnsw, HnswParams, Metric, SplitMix64};

fn gen_vectors(seed: u64, n: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = SplitMix64::new(seed);
    (0..n)
        .map(|_| (0..dim).map(|_| rng.next_f64() as f32).collect())
        .collect()
}

fn brute_force(query: &[f32], vectors: &[Vec<f32>], k: usize) -> Vec<u32> {
    let mut scored: Vec<(f32, u32)> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (distance(Metric::L2, query, v), i as u32))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(k).map(|(_, i)| i).collect()
}

fn recall_of<F: Fn(&[f32]) -> Vec<(u32, f32)>>(
    queries: &[Vec<f32>],
    data: &[Vec<f32>],
    k: usize,
    search: F,
) -> f64 {
    let mut total = 0.0;
    for q in queries {
        let truth: std::collections::HashSet<u32> = brute_force(q, data, k).into_iter().collect();
        let got = search(q);
        let hits = got.iter().filter(|(id, _)| truth.contains(id)).count();
        total += hits as f64 / k as f64;
    }
    total / queries.len() as f64
}

#[test]
fn quantized_recall_within_2_percent_of_full_precision() {
    let dim = 24;
    let n = 1500;
    let k = 10;
    let ef = 100;

    let data = gen_vectors(1, n, dim);
    let mut index = Hnsw::new(
        dim,
        HnswParams {
            m: 16,
            ef_construction: 200,
            seed: 42,
            metric: Metric::L2,
        },
    );
    for v in &data {
        index.insert(v.clone());
    }
    index.quantize();

    let queries = gen_vectors(999, 40, dim);
    let recall_f32 = recall_of(&queries, &data, k, |q| index.search(q, k, ef));
    let recall_q = recall_of(&queries, &data, k, |q| index.search_quantized(q, k, ef));

    eprintln!("recall@{k}: f32={recall_f32:.4}  quantized+rerank={recall_q:.4}");
    assert!(recall_q >= 0.95, "quantized recall {recall_q:.3} too low");
    assert!(
        recall_q >= recall_f32 - 0.02,
        "quantized recall {recall_q:.3} is >2% below f32 {recall_f32:.3}"
    );
}

#[test]
fn rerank_makes_results_exactly_sorted() {
    let data = gen_vectors(7, 400, 16);
    let mut index = Hnsw::new(16, HnswParams::default());
    for v in &data {
        index.insert(v.clone());
    }
    index.quantize();

    let res = index.search_quantized(&data[3], 10, 80);
    // Distances reported are exact f32 (from rerank), so strictly ascending.
    for w in res.windows(2) {
        assert!(w[0].1 <= w[1].1);
    }
    // The nearest to a stored vector is itself (distance ~0).
    assert_eq!(res[0].0, 3);
    assert!(res[0].1.abs() < 1e-6);
}
