//! TextView::parse + bm25/postings must never panic on malformed bytes.

use ll_text::{serialize_text_index, InvertedIndex, TextView};

struct Rng(u64);
impl Rng {
    fn new(s: u64) -> Self {
        Rng(s ^ 0xA5A5)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z ^ (z >> 31)
    }
}

fn valid_bytes() -> Vec<u8> {
    let docs = vec![
        Some("the quick brown fox"),
        Some("quick volatility hedging"),
        None,
        Some("lazy dog sleeps"),
    ];
    let idx = InvertedIndex::build(&docs);
    serialize_text_index(&idx, 1)
}

fn probe(bytes: &[u8]) {
    if let Ok(view) = TextView::parse(bytes) {
        let _ = view.bm25("quick volatility", 10);
        let _ = view.postings("quick");
        let _ = view.doc_len(0);
        let _ = view.doc_len(9_999_999);
    }
}

#[test]
fn parse_and_query_never_panic() {
    let base = valid_bytes();
    let mut r = Rng::new(3);
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
