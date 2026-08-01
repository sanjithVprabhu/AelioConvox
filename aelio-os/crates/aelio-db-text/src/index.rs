//! In-memory inverted index + BM25 scoring and boolean matching.
//!
//! Documents are identified by `local_offset` (0..num_docs). A null document (no text)
//! contributes no postings and has length 0.

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::tokenize::tokenize;

/// BM25 term-frequency saturation.
pub const BM25_K1: f32 = 1.2;
/// BM25 length-normalization.
pub const BM25_B: f32 = 0.75;

/// An in-memory inverted index over a text column.
#[derive(Debug, Clone)]
pub struct InvertedIndex {
    num_docs: usize,
    total_tokens: u64,
    /// token count per document (indexed by `local_offset`).
    doc_len: Vec<u32>,
    /// term → postings sorted by doc: `(local_offset, term_frequency)`.
    postings: BTreeMap<String, Vec<(u32, u32)>>,
}

impl InvertedIndex {
    /// Build from documents indexed by `local_offset` (`None` = null/no text).
    pub fn build(docs: &[Option<&str>]) -> Self {
        let num_docs = docs.len();
        let mut doc_len = vec![0u32; num_docs];
        let mut total_tokens = 0u64;
        let mut postings: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();

        for (doc, maybe_text) in docs.iter().enumerate() {
            let Some(text) = maybe_text else { continue };
            let tokens = tokenize(text);
            doc_len[doc] = tokens.len() as u32;
            total_tokens += tokens.len() as u64;
            // term-frequency within this document
            let mut tf: HashMap<String, u32> = HashMap::new();
            for t in tokens {
                *tf.entry(t).or_insert(0) += 1;
            }
            for (term, freq) in tf {
                // docs are processed in increasing order, so postings stay doc-sorted.
                postings.entry(term).or_default().push((doc as u32, freq));
            }
        }

        InvertedIndex {
            num_docs,
            total_tokens,
            doc_len,
            postings,
        }
    }

    pub fn num_docs(&self) -> usize {
        self.num_docs
    }
    pub fn num_terms(&self) -> usize {
        self.postings.len()
    }
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens
    }
    pub fn doc_len(&self, doc: u32) -> u32 {
        self.doc_len.get(doc as usize).copied().unwrap_or(0)
    }

    /// Postings for a term (already tokenized/normalized), or empty.
    pub fn postings(&self, term: &str) -> &[(u32, u32)] {
        self.postings.get(term).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Iterate `(term, postings)` in sorted term order (for serialization).
    pub fn iter_terms(&self) -> impl Iterator<Item = (&str, &[(u32, u32)])> {
        self.postings
            .iter()
            .map(|(t, p)| (t.as_str(), p.as_slice()))
    }

    fn avgdl(&self) -> f32 {
        if self.num_docs == 0 {
            0.0
        } else {
            self.total_tokens as f32 / self.num_docs as f32
        }
    }

    /// BM25 top-`k` documents for a free-text query, descending by score.
    pub fn bm25(&self, query: &str, k: usize) -> Vec<(u32, f32)> {
        let avgdl = self.avgdl();
        let mut scores: HashMap<u32, f32> = HashMap::new();
        for term in dedup(tokenize(query)) {
            let posting = self.postings(&term);
            if posting.is_empty() {
                continue;
            }
            let idf = bm25_idf(self.num_docs as f32, posting.len() as f32);
            for &(doc, tf) in posting {
                let dl = self.doc_len(doc) as f32;
                *scores.entry(doc).or_insert(0.0) += bm25_term(idf, tf as f32, dl, avgdl);
            }
        }
        top_k(scores, k)
    }

    /// Documents containing **all** terms (boolean AND), ascending by doc id.
    pub fn match_all(&self, terms: &[&str]) -> Vec<u32> {
        if terms.is_empty() {
            return Vec::new();
        }
        let mut acc: Option<Vec<u32>> = None;
        for t in terms {
            let docs: Vec<u32> = self.postings(t).iter().map(|&(d, _)| d).collect();
            acc = Some(match acc {
                None => docs,
                Some(prev) => intersect_sorted(&prev, &docs),
            });
            if acc.as_ref().unwrap().is_empty() {
                break;
            }
        }
        acc.unwrap_or_default()
    }

    /// Documents containing **any** term (boolean OR), ascending by doc id.
    pub fn match_any(&self, terms: &[&str]) -> Vec<u32> {
        let mut set: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        for t in terms {
            for &(d, _) in self.postings(t) {
                set.insert(d);
            }
        }
        set.into_iter().collect()
    }
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

pub(crate) fn bm25_idf(n: f32, df: f32) -> f32 {
    ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
}

pub(crate) fn bm25_term(idf: f32, tf: f32, dl: f32, avgdl: f32) -> f32 {
    let norm = if avgdl > 0.0 { dl / avgdl } else { 1.0 };
    idf * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * norm))
}

pub(crate) fn top_k(scores: HashMap<u32, f32>, k: usize) -> Vec<(u32, f32)> {
    let mut v: Vec<(u32, f32)> = scores.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(k);
    v
}

fn intersect_sorted(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<Option<&'static str>> {
        vec![
            Some("the quick brown fox"),
            Some("the lazy dog"),
            Some("quick quick derivatives and volatility"),
            None,
            Some("volatility hedging strategies"),
        ]
    }

    #[test]
    fn postings_are_correct() {
        let idx = InvertedIndex::build(&corpus());
        assert_eq!(idx.num_docs(), 5);
        assert_eq!(idx.postings("quick"), &[(0, 1), (2, 2)]);
        assert_eq!(idx.postings("the"), &[(0, 1), (1, 1)]);
        assert!(idx.postings("missing").is_empty());
        assert_eq!(idx.doc_len(3), 0); // null doc
    }

    #[test]
    fn boolean_match() {
        let idx = InvertedIndex::build(&corpus());
        assert_eq!(idx.match_all(&["quick", "volatility"]), vec![2]);
        assert_eq!(idx.match_any(&["dog", "hedging"]), vec![1, 4]);
        assert!(idx.match_all(&["quick", "dog"]).is_empty());
    }

    #[test]
    fn bm25_ranks_rarer_and_denser_terms_higher() {
        let idx = InvertedIndex::build(&corpus());
        // "volatility" appears in docs 2 and 4; doc 4 is shorter, doc 2 has more tokens.
        let res = idx.bm25("volatility", 10);
        let docs: Vec<u32> = res.iter().map(|(d, _)| *d).collect();
        assert!(docs.contains(&2) && docs.contains(&4));
        // Shorter doc 4 should outrank longer doc 2 for a single occurrence each.
        let pos4 = docs.iter().position(|&d| d == 4).unwrap();
        let pos2 = docs.iter().position(|&d| d == 2).unwrap();
        assert!(pos4 < pos2, "shorter doc should rank higher: {docs:?}");
    }
}
