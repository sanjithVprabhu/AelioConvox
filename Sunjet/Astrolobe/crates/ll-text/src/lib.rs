//! `ll-text` — LL's full-text index.
//!
//! Tokenize → inverted index → BM25 scoring + boolean matching, with an on-disk `Text`
//! section that embeds in a `.vss` file (framed by `ll-format` as an opaque section). v0
//! uses a sorted term dictionary (binary search) and delta+varint postings; an FST term
//! dictionary and PFOR-Delta/skip-list postings are later optimizations.

mod index;
mod page;
mod tokenize;

pub use index::{InvertedIndex, BM25_B, BM25_K1};
pub use page::{serialize_text_index, TextError, TextView};
pub use tokenize::tokenize;
