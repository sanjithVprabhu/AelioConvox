//! Serialize an HNSW to bytes (with BFS reorder), search over the bytes, and confirm
//! recall is preserved after the graph → bytes → graph round-trip.

use aelio_db_index::{distance, serialize_hnsw, Hnsw, HnswParams, HnswView, Metric, SplitMix64};

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

#[test]
fn serialized_search_preserves_recall() {
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

    // local_offset == node id for this test (identity mapping).
    let local_offsets: Vec<u32> = (0..n as u32).collect();
    let bytes = serialize_hnsw(&index, 1, &local_offsets);
    let view = HnswView::parse(&bytes).expect("parse");
    assert_eq!(view.len(), n);
    assert_eq!(view.dim(), dim);

    let queries = gen_vectors(999, 40, dim);
    let mut total = 0.0;
    for q in &queries {
        let truth: std::collections::HashSet<u32> = brute_force(q, &data, k).into_iter().collect();
        // Search over the serialized bytes, then rerank candidates with exact f32.
        let mut cands = view.search(q, ef);
        cands.sort_by(|a, b| {
            let da = distance(Metric::L2, q, &data[a.local_offset as usize]);
            let db = distance(Metric::L2, q, &data[b.local_offset as usize]);
            da.total_cmp(&db)
        });
        let hits = cands
            .iter()
            .take(k)
            .filter(|nb| truth.contains(&nb.local_offset))
            .count();
        total += hits as f64 / k as f64;
    }
    let recall = total / queries.len() as f64;
    eprintln!("serialized recall@{k} = {recall:.4}");
    assert!(recall >= 0.95, "serialized recall {recall:.3} too low");
}

#[test]
fn bfs_reorder_is_a_valid_permutation() {
    // Node 0's local_offset must survive reordering: after BFS the slot for whatever
    // hnsw_node_id maps to old node 7 must report local_offset 7 (identity mapping),
    // and every local_offset 0..n must appear exactly once across all slots.
    let dim = 8;
    let n = 200;
    let data = gen_vectors(5, n, dim);
    let mut index = Hnsw::new(dim, HnswParams::default());
    for v in &data {
        index.insert(v.clone());
    }
    index.quantize();
    let local_offsets: Vec<u32> = (0..n as u32).collect();
    let bytes = serialize_hnsw(&index, 0, &local_offsets);
    let view = HnswView::parse(&bytes).unwrap();

    // Each query that exactly matches a stored vector should find that row first.
    for probe in [0usize, 7, 99, 199] {
        let mut cands = view.search(&data[probe], 32);
        cands.sort_by(|a, b| {
            let da = distance(Metric::L2, &data[probe], &data[a.local_offset as usize]);
            let db = distance(Metric::L2, &data[probe], &data[b.local_offset as usize]);
            da.total_cmp(&db)
        });
        assert_eq!(cands[0].local_offset, probe as u32);
    }
}

#[test]
fn empty_index_serializes_and_parses() {
    let index = {
        let mut h = Hnsw::new(4, HnswParams::default());
        h.quantize();
        h
    };
    let bytes = serialize_hnsw(&index, 3, &[]);
    let view = HnswView::parse(&bytes).unwrap();
    assert!(view.is_empty());
    assert!(view.search(&[0.0, 0.0, 0.0, 0.0], 10).is_empty());
}
