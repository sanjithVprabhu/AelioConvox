//! The `Source` abstraction: a queryable unit that answers per-modality searches and
//! returns `(global_row_id, score)` candidates at a snapshot LSN.
//!
//! Both the in-memory [`Memtable`] and (later) each flushed `.vss` file implement this, so
//! the planner can fan a sub-query out to every source and merge by `row_id` — the global
//! id (D-001) is the universal join key across recent and persisted data, and across
//! modalities. This is what makes "one query plan" deliver read-your-writes.
//!
//! v0 memtable implementation scans the visible rows (correct MVCC for free, and fast at
//! memtable scale — the spec's brute-force-in-memtable position). Precomputed incremental
//! analogs (flat vector array, inverted index, edge maps) are a later optimization.

use std::collections::HashMap;

use ll_engine::{Memtable, Value};
use ll_text::tokenize;

use crate::util::{l2_sq, top_k};

/// BM25 constants (match `ll-text`).
const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;

/// A queryable source of rows, filtered to a snapshot LSN. `Sync` so the executor can
/// fan distance work across threads.
pub trait Source: Sync {
    /// k nearest neighbors of `query` in vector column `col`, ascending by squared-L2,
    /// searching the index at breadth `ef` (the planner picks `ef` to hit a recall target;
    /// exact sources like the memtable ignore it).
    fn vector_search(&self, col: u32, query: &[f32], k: usize, ef: usize, snapshot: u64) -> Vec<(u64, f32)>;
    /// BM25 top-k for `query` over text column `col`, descending by score.
    fn text_search(&self, col: u32, query: &str, k: usize, snapshot: u64) -> Vec<(u64, f32)>;
    /// Global target ids of `src`'s edges in column `col` (empty if `src` isn't a live row
    /// here). Targets that live in another source are passed through — edges carry global
    /// ids, so the executor resolves each target's visibility across all sources, enabling
    /// cross-file traversal. Only targets known-deleted *in this source* are dropped.
    fn out_neighbors(&self, col: u32, src: u64, snapshot: u64) -> Vec<u64>;
    /// Sources pointing at `tgt` in edge column `col`.
    fn in_neighbors(&self, col: u32, tgt: u64, snapshot: u64) -> Vec<u64>;
    /// A row's scalar value (for predicates and rerank).
    fn scalar(&self, row_id: u64, col: u32, snapshot: u64) -> Option<Value>;
    /// Visible row ids in this source (for pre-filter and scan).
    fn all_rows(&self, snapshot: u64) -> Vec<u64>;
    /// A row's full-precision vector for a column (for exact pre-filter distance).
    fn vector(&self, col: u32, row_id: u64, snapshot: u64) -> Option<Vec<f32>>;

    /// The newest version of `row_id` this source holds, visible at `snapshot`, as
    /// `(stamp, deleted)`: `stamp` is the version's creation LSN (globally monotonic, so
    /// higher = newer) and `deleted` marks a tombstone. `None` if this source has no
    /// visible version. The executor compares stamps across sources to resolve
    /// newest-version-wins, so an updated/deleted row suppresses its stale copy elsewhere.
    fn version_at(&self, row_id: u64, snapshot: u64) -> Option<(u64, bool)>;

    /// This source's measured unfiltered recall@k as a function of search breadth `ef`, for
    /// vector column `col` — `(ef, recall)` pairs in ascending `ef`. Empty means exact (e.g.
    /// the memtable scans), which the executor treats as recall 1.0 at any `ef`. The planner
    /// uses the per-`ef` minimum across sources to pick the smallest `ef` meeting the target,
    /// or to fall back to exact pre-filter when none does.
    fn ann_recall_curve(&self, _col: u32) -> Vec<(usize, f64)> {
        Vec::new()
    }

    /// Rows satisfying all `preds` via a scalar index, in O(log N + matches), if every
    /// predicate is on an indexed column with a supported op; otherwise `None` (the caller
    /// falls back to a scan). Default: no index.
    fn rows_for_predicates(
        &self,
        _preds: &[crate::exec::Predicate],
        _snapshot: u64,
    ) -> Option<Vec<u64>> {
        None
    }
}

impl Source for Memtable {
    fn vector_search(&self, col: u32, query: &[f32], k: usize, _ef: usize, snapshot: u64) -> Vec<(u64, f32)> {
        // The memtable scans every visible row — exact, so `ef` is irrelevant.
        let mut scored = Vec::new();
        for (id, row) in self.scan(snapshot) {
            if let Some(Value::Vector(v)) = row.get(&col) {
                if v.len() == query.len() {
                    scored.push((id, l2_sq(query, v)));
                }
            }
        }
        top_k(scored, k, true)
    }

    fn text_search(&self, col: u32, query: &str, k: usize, snapshot: u64) -> Vec<(u64, f32)> {
        // Visible documents for this column.
        let docs: Vec<(u64, Vec<String>)> = self
            .scan(snapshot)
            .into_iter()
            .filter_map(|(id, row)| match row.get(&col) {
                Some(Value::Utf8(s)) => Some((id, tokenize(s))),
                _ => None,
            })
            .collect();
        let n = docs.len() as f32;
        if n == 0.0 {
            return Vec::new();
        }
        let total_tokens: usize = docs.iter().map(|(_, t)| t.len()).sum();
        let avgdl = total_tokens as f32 / n;

        let mut qterms = tokenize(query);
        qterms.sort();
        qterms.dedup();

        let mut scores: HashMap<u64, f32> = HashMap::new();
        for term in qterms {
            let df = docs.iter().filter(|(_, t)| t.contains(&term)).count() as f32;
            if df == 0.0 {
                continue;
            }
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for (id, toks) in &docs {
                let tf = toks.iter().filter(|x| **x == term).count() as f32;
                if tf == 0.0 {
                    continue;
                }
                let dl = toks.len() as f32;
                let s = idf * (tf * (BM25_K1 + 1.0))
                    / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avgdl));
                *scores.entry(*id).or_insert(0.0) += s;
            }
        }
        top_k(scores.into_iter().collect(), k, false)
    }

    fn out_neighbors(&self, col: u32, src: u64, snapshot: u64) -> Vec<u64> {
        let Some(row) = self.get(src, snapshot) else {
            return Vec::new();
        };
        match row.get(&col) {
            Some(Value::Edges(ts)) => ts
                .iter()
                .copied()
                .filter(|t| match self.version_at(*t, snapshot) {
                    Some((_, deleted)) => !deleted, // in the memtable: drop if deleted here
                    None => true, // target lives in another source — the executor resolves it
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn in_neighbors(&self, col: u32, tgt: u64, snapshot: u64) -> Vec<u64> {
        self.scan(snapshot)
            .into_iter()
            .filter_map(|(id, row)| match row.get(&col) {
                Some(Value::Edges(ts)) if ts.contains(&tgt) => Some(id),
                _ => None,
            })
            .collect()
    }

    fn scalar(&self, row_id: u64, col: u32, snapshot: u64) -> Option<Value> {
        self.get(row_id, snapshot).and_then(|r| r.get(&col).cloned())
    }

    fn all_rows(&self, snapshot: u64) -> Vec<u64> {
        self.scan(snapshot).into_iter().map(|(id, _)| id).collect()
    }

    fn vector(&self, col: u32, row_id: u64, snapshot: u64) -> Option<Vec<f32>> {
        match self.get(row_id, snapshot)?.get(&col)? {
            Value::Vector(v) => Some(v.clone()),
            _ => None,
        }
    }

    fn version_at(&self, row_id: u64, snapshot: u64) -> Option<(u64, bool)> {
        Memtable::version_at(self, row_id, snapshot)
    }
}
