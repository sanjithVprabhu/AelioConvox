//! EdgeView::parse + traversal must never panic on malformed bytes (incl. bloom edge cases).

use ll_graph::{serialize_edge_index, EdgeIndex, EdgeView};

struct Rng(u64);
impl Rng {
    fn new(s: u64) -> Self {
        Rng(s ^ 0x1234)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z ^ (z >> 31)
    }
}

fn valid_bytes() -> Vec<u8> {
    let edges = vec![(0u32, 1u64), (0, 2), (1, 3), (2, 3), (3, 4), (4, 0)];
    let idx = EdgeIndex::build(&edges, 5);
    serialize_edge_index(&idx, 1)
}

fn probe(bytes: &[u8]) {
    if let Ok(view) = EdgeView::parse(bytes) {
        let _ = view.out_neighbors(0);
        let _ = view.out_neighbors(9_999_999);
        let _ = view.in_neighbors(3);
        let _ = view.might_target(3);
        let _ = view.traverse_forward(&[0], 3, |t| Some(t as u32));
    }
}

#[test]
fn parse_and_traverse_never_panic() {
    let base = valid_bytes();
    let mut r = Rng::new(5);
    for _ in 0..500 {
        let len = (r.next() % 200) as usize;
        let b: Vec<u8> = (0..len).map(|_| (r.next() % 256) as u8).collect();
        probe(&b);
    }
    for len in 0..base.len() {
        probe(&base[..len]);
    }
    for _ in 0..2000 {
        let mut b = base.clone();
        let flips = 1 + (r.next() % 4) as usize;
        for _ in 0..flips {
            let i = (r.next() as usize) % b.len().max(1);
            if i < b.len() {
                b[i] ^= 1 + (r.next() % 255) as u8;
            }
        }
        probe(&b);
    }
}
