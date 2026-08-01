//! The serialized Text section reproduces the in-memory index's postings and BM25 ranking.

use aelio_db_text::{serialize_text_index, InvertedIndex, TextView};

fn corpus() -> Vec<Option<&'static str>> {
    vec![
        Some("the quick brown fox jumps"),
        Some("the lazy dog sleeps"),
        Some("quick quick derivatives and volatility hedging"),
        None,
        Some("volatility hedging strategies for derivatives"),
        Some("a brown fox and a brown dog"),
    ]
}

#[test]
fn view_matches_in_memory() {
    let idx = InvertedIndex::build(&corpus());
    let bytes = serialize_text_index(&idx, 7);
    let view = TextView::parse(&bytes).expect("parse");

    assert_eq!(view.num_docs(), idx.num_docs());
    assert_eq!(view.num_terms(), idx.num_terms());

    // Postings round-trip for several terms (present and absent).
    for term in [
        "the",
        "quick",
        "brown",
        "volatility",
        "derivatives",
        "missing",
    ] {
        assert_eq!(
            view.postings(term),
            idx.postings(term).to_vec(),
            "term {term}"
        );
    }

    // doc_len round-trips, including the null doc.
    for d in 0..idx.num_docs() as u32 {
        assert_eq!(view.doc_len(d), idx.doc_len(d));
    }

    // BM25 ranking is identical between in-memory and on-disk.
    for q in ["volatility hedging", "brown fox", "derivatives"] {
        assert_eq!(view.bm25(q, 5), idx.bm25(q, 5), "query {q}");
    }
}

#[test]
fn empty_corpus_serializes() {
    let idx = InvertedIndex::build(&[]);
    let bytes = serialize_text_index(&idx, 0);
    let view = TextView::parse(&bytes).unwrap();
    assert_eq!(view.num_docs(), 0);
    assert_eq!(view.num_terms(), 0);
    assert!(view.bm25("anything", 10).is_empty());
}
