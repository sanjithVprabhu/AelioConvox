//! HnswView::parse + search must never panic on malformed bytes (random, mutated, truncated).

use aelio_db_index::{serialize_hnsw, Hnsw, HnswParams, HnswView, SplitMix64};

fn valid_bytes() -> Vec<u8> {
    let mut h = Hnsw::new(8, HnswParams::default());
    let mut rng = SplitMix64::new(1);
    for _ in 0..50 {
        h.insert((0..8).map(|_| rng.next_f64() as f32).collect());
    }
    h.quantize();
    serialize_hnsw(&h, 1, &(0..50u32).collect::<Vec<_>>())
}

fn probe(bytes: &[u8]) {
    if let Ok(view) = HnswView::parse(bytes) {
        // Querying a successfully-parsed (but possibly corrupt) view must not panic.
        let _ = view.search(&[0.0; 8], 10);
        let _ = view.search_filtered(&[0.0; 8], 10, |id| id % 2 == 0);
    }
}

#[test]
fn parse_and_search_never_panic() {
    let base = valid_bytes();
    let mut rng = SplitMix64::new(7);

    // random bytes
    for _ in 0..500 {
        let len = (rng.next_f64() * 200.0) as usize;
        let b: Vec<u8> = (0..len).map(|_| (rng.next_f64() * 256.0) as u8).collect();
        probe(&b);
    }
    // truncations
    for len in 0..base.len() {
        probe(&base[..len]);
    }
    // mutations
    for _ in 0..2000 {
        let mut b = base.clone();
        let flips = 1 + (rng.next_f64() * 4.0) as usize;
        for _ in 0..flips {
            let i = (rng.next_f64() * b.len() as f64) as usize % b.len().max(1);
            if i < b.len() {
                b[i] ^= 1 + (rng.next_f64() * 255.0) as u8;
            }
        }
        probe(&b);
    }
}
