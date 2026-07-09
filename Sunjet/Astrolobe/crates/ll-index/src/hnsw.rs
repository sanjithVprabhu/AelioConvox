//! In-memory HNSW (Malkov & Yashunin, 2016): hierarchical navigable small-world graph
//! for approximate nearest-neighbor search.
//!
//! Steps so far: build + f32 search (step 1); 8-bit quantized traversal + full-precision
//! rerank (step 2). The on-disk page layout and BFS reordering are later steps. Node ids
//! are insertion order; they become `hnsw_node_id` (BFS-reordered) when serialized.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::HashSet;

use crate::quant::ScalarQuantizer;
use crate::rng::SplitMix64;

/// Distance metric. L2 returns *squared* Euclidean distance (monotonic with true L2, so
/// ordering and recall are unaffected); cosine returns `1 - cosine_similarity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    L2,
    Cosine,
}

/// Distance between two equal-length vectors under `metric`.
pub fn distance(metric: Metric, a: &[f32], b: &[f32]) -> f32 {
    match metric {
        Metric::L2 => crate::simd::l2_squared(a, b),
        Metric::Cosine => {
            let mut dot = 0f32;
            let mut na = 0f32;
            let mut nb = 0f32;
            for (x, y) in a.iter().zip(b) {
                dot += x * y;
                na += x * x;
                nb += y * y;
            }
            1.0 - dot / (na.sqrt() * nb.sqrt() + 1e-12)
        }
    }
}

/// A candidate (node id + distance to the query), totally ordered by `(dist, id)`.
#[derive(Debug, Clone, Copy)]
struct Cand {
    dist: f32,
    id: u32,
}

impl PartialEq for Cand {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.dist.to_bits() == other.dist.to_bits()
    }
}
impl Eq for Cand {}
impl Ord for Cand {
    fn cmp(&self, other: &Self) -> Ordering {
        self.dist.total_cmp(&other.dist).then(self.id.cmp(&other.id))
    }
}
impl PartialOrd for Cand {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Build/search parameters.
#[derive(Debug, Clone, Copy)]
pub struct HnswParams {
    /// Max neighbors per node per upper layer (layer 0 uses `2*m`).
    pub m: usize,
    /// Beam width during construction.
    pub ef_construction: usize,
    /// PRNG seed for level assignment (determinism).
    pub seed: u64,
    pub metric: Metric,
}

impl Default for HnswParams {
    fn default() -> Self {
        HnswParams {
            m: 16,
            ef_construction: 200,
            seed: 0x5EED,
            metric: Metric::L2,
        }
    }
}

/// An in-memory HNSW index over `dim`-dimensional vectors.
#[derive(Debug)]
pub struct Hnsw {
    dim: usize,
    m: usize,
    m0: usize,
    ef_construction: usize,
    metric: Metric,
    level_mult: f64,
    rng: SplitMix64,
    vectors: Vec<Vec<f32>>,
    levels: Vec<usize>,
    links: Vec<Vec<Vec<u32>>>,
    entry: Option<u32>,
    max_level: usize,
    /// Set by [`Hnsw::quantize`]; enables [`Hnsw::search_quantized`].
    quantizer: Option<ScalarQuantizer>,
    quantized: Vec<Vec<u8>>,
}

const MAX_LEVEL_CAP: usize = 16;

impl Hnsw {
    pub fn new(dim: usize, params: HnswParams) -> Self {
        Hnsw {
            dim,
            m: params.m,
            m0: params.m * 2,
            ef_construction: params.ef_construction,
            metric: params.metric,
            level_mult: 1.0 / (params.m as f64).ln(),
            rng: SplitMix64::new(params.seed),
            vectors: Vec::new(),
            levels: Vec::new(),
            links: Vec::new(),
            entry: None,
            max_level: 0,
            quantizer: None,
            quantized: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.vectors.len()
    }
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
    pub fn dim(&self) -> usize {
        self.dim
    }

    // ---- accessors for serialization (ll-index page layout) ----
    pub fn m(&self) -> usize {
        self.m
    }
    pub fn m0(&self) -> usize {
        self.m0
    }
    pub fn max_level(&self) -> usize {
        self.max_level
    }
    pub fn entry(&self) -> Option<u32> {
        self.entry
    }
    pub fn node_level(&self, id: u32) -> usize {
        self.levels[id as usize]
    }
    pub fn neighbors(&self, id: u32, layer: usize) -> &[u32] {
        &self.links[id as usize][layer]
    }
    /// The 8-bit code for a node (requires [`Hnsw::quantize`]).
    pub fn code(&self, id: u32) -> &[u8] {
        &self.quantized[id as usize]
    }
    pub fn quantizer(&self) -> Option<&ScalarQuantizer> {
        self.quantizer.as_ref()
    }

    fn random_level(&mut self) -> usize {
        let u = 1.0 - self.rng.next_f64(); // (0, 1]
        let level = (-u.ln() * self.level_mult).floor() as usize;
        level.min(MAX_LEVEL_CAP)
    }

    /// Insert a vector, returning its node id (insertion order).
    ///
    /// # Panics
    /// If `vector.len() != dim`. (Call before [`Hnsw::quantize`].)
    pub fn insert(&mut self, vector: Vec<f32>) -> u32 {
        assert_eq!(vector.len(), self.dim, "vector dimension mismatch");
        let level = self.random_level();

        if self.entry.is_none() {
            self.vectors.push(vector);
            self.levels.push(level);
            self.links.push(vec![Vec::new(); level + 1]);
            self.entry = Some(0);
            self.max_level = level;
            return 0;
        }

        // --- Phase 1: search (immutable). Distance is to the new `vector`, in f32. ---
        let metric = self.metric;
        let node_dist = |id: u32| distance(metric, &vector, &self.vectors[id as usize]);

        let mut ep = self.entry.unwrap();
        let ep_level = self.max_level;
        for lc in (level + 1..=ep_level).rev() {
            let w = self.search_layer(&node_dist, &[ep], 1, lc);
            if let Some(best) = w.first() {
                ep = best.id;
            }
        }

        let start = level.min(ep_level);
        let mut per_level: Vec<(usize, Vec<u32>)> = Vec::new();
        for lc in (0..=start).rev() {
            let w = self.search_layer(&node_dist, &[ep], self.ef_construction, lc);
            let m = if lc == 0 { self.m0 } else { self.m };
            let neighbors = select_neighbors_heuristic(self.metric, &self.vectors, &w, m);
            if let Some(best) = w.first() {
                ep = best.id;
            }
            per_level.push((lc, neighbors));
        }

        // --- Phase 2: mutate ---
        let id = self.vectors.len() as u32;
        self.vectors.push(vector);
        self.levels.push(level);
        self.links.push(vec![Vec::new(); level + 1]);

        let m0 = self.m0;
        let m = self.m;
        let Hnsw {
            ref vectors,
            ref mut links,
            ..
        } = *self;
        for (lc, neighbors) in &per_level {
            links[id as usize][*lc] = neighbors.clone();
            for &nb in neighbors {
                links[nb as usize][*lc].push(id);
                let maxconn = if *lc == 0 { m0 } else { m };
                if links[nb as usize][*lc].len() > maxconn {
                    let base = &vectors[nb as usize];
                    let mut cands: Vec<Cand> = links[nb as usize][*lc]
                        .iter()
                        .map(|&x| Cand {
                            dist: distance(metric, base, &vectors[x as usize]),
                            id: x,
                        })
                        .collect();
                    cands.sort();
                    links[nb as usize][*lc] =
                        select_neighbors_heuristic(metric, vectors, &cands, maxconn);
                }
            }
        }

        if level > self.max_level {
            self.max_level = level;
            self.entry = Some(id);
        }
        id
    }

    /// Train the 8-bit quantizer over the current vectors (call after all inserts).
    /// Required before [`Hnsw::search_quantized`]. L2 metric only for now.
    pub fn quantize(&mut self) {
        assert_eq!(
            self.metric,
            Metric::L2,
            "quantized search currently supports the L2 metric only"
        );
        let q = ScalarQuantizer::train(&self.vectors, self.dim);
        self.quantized = self.vectors.iter().map(|v| q.quantize(v)).collect();
        self.quantizer = Some(q);
    }

    pub fn is_quantized(&self) -> bool {
        self.quantizer.is_some()
    }

    /// Approximate `k` nearest neighbors of `query` using exact f32 distances throughout.
    pub fn search(&self, query: &[f32], k: usize, ef_search: usize) -> Vec<(u32, f32)> {
        assert_eq!(query.len(), self.dim, "query dimension mismatch");
        let metric = self.metric;
        let node_dist = |id: u32| distance(metric, query, &self.vectors[id as usize]);
        self.search_with(&node_dist, k, ef_search)
            .into_iter()
            .map(|c| (c.id, c.dist))
            .collect()
    }

    /// Estimate this index's unfiltered recall@`k` at each `ef` in `ef_ladder`, sampling up to
    /// `sample` of its own vectors as queries and searching via the quantized path (the same
    /// path the on-disk view uses). 1.0 means the ANN returns the exact top-k. Brute-force
    /// truth is computed once per sampled query and reused across all `ef` values. The planner
    /// stores this curve so it can pick the smallest `ef` that meets a recall target — or fall
    /// back to exact pre-filter when no `ef` does (recall falls with dimensionality). Returns
    /// `(ef, recall)` in ladder order. Requires `quantize()`.
    pub fn measure_recall_curve(&self, k: usize, ef_ladder: &[usize], sample: usize) -> Vec<(usize, f64)> {
        let n = self.vectors.len();
        if k == 0 || n <= k + 1 || self.quantizer.is_none() {
            return ef_ladder.iter().map(|&ef| (ef, 1.0)).collect();
        }
        let metric = self.metric;
        let step = (n / sample.max(1)).max(1);
        let mut sums = vec![0.0f64; ef_ladder.len()];
        let mut count = 0usize;
        let mut i = 0;
        while i < n {
            let qi = i as u32;
            let q = &self.vectors[i];
            let mut d: Vec<(f32, u32)> = self
                .vectors
                .iter()
                .enumerate()
                .map(|(j, v)| (distance(metric, q, v), j as u32))
                .collect();
            d.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            // Exclude the query's own point from the ground truth: a dataset point is a
            // trivial self-match, which would inflate recall vs real out-of-sample queries.
            let truth: std::collections::HashSet<u32> =
                d.iter().map(|&(_, j)| j).filter(|&j| j != qi).take(k).collect();
            for (li, &ef) in ef_ladder.iter().enumerate() {
                // Fetch k+1 and drop self, so the self-slot doesn't crowd out a true neighbor.
                let got = self.search_quantized(q, k + 1, ef);
                let hits = got
                    .iter()
                    .filter(|(id, _)| *id != qi && truth.contains(id))
                    .count();
                sums[li] += hits as f64 / k as f64;
            }
            count += 1;
            i += step;
        }
        ef_ladder
            .iter()
            .enumerate()
            .map(|(li, &ef)| (ef, if count == 0 { 1.0 } else { sums[li] / count as f64 }))
            .collect()
    }

    /// Recall@`k` at a single `ef` (convenience over [`Hnsw::measure_recall_curve`]).
    pub fn measure_recall(&self, k: usize, ef_search: usize, sample: usize) -> f64 {
        self.measure_recall_curve(k, &[ef_search], sample)
            .first()
            .map(|&(_, r)| r)
            .unwrap_or(1.0)
    }

    /// Approximate `k` nearest neighbors using **8-bit quantized traversal** followed by
    /// **full-precision rerank** of the candidate set. Requires [`Hnsw::quantize`].
    ///
    /// # Panics
    /// If `quantize()` has not been called.
    pub fn search_quantized(&self, query: &[f32], k: usize, ef_search: usize) -> Vec<(u32, f32)> {
        assert_eq!(query.len(), self.dim, "query dimension mismatch");
        let q = self
            .quantizer
            .as_ref()
            .expect("call quantize() before search_quantized()");
        let qquery = q.quantize(query);
        let node_dist = |id: u32| q.l2_quantized(&qquery, &self.quantized[id as usize]);

        // Traverse in quantized space, gathering the ef candidate set.
        let candidates = self.search_with(&node_dist, ef_search.max(k), ef_search);

        // Rerank candidates with exact f32 distance, then take top-k.
        let metric = self.metric;
        let mut reranked: Vec<(u32, f32)> = candidates
            .into_iter()
            .map(|c| (c.id, distance(metric, query, &self.vectors[c.id as usize])))
            .collect();
        reranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        reranked.truncate(k);
        reranked
    }

    /// Shared search skeleton: greedy descent to layer 1, then a layer-0 beam of `ef`.
    /// Returns up to `top` candidates ascending by the supplied distance.
    fn search_with<F: Fn(u32) -> f32>(&self, node_dist: &F, top: usize, ef: usize) -> Vec<Cand> {
        let Some(mut ep) = self.entry else {
            return Vec::new();
        };
        for lc in (1..=self.max_level).rev() {
            let w = self.search_layer(node_dist, &[ep], 1, lc);
            if let Some(best) = w.first() {
                ep = best.id;
            }
        }
        let mut w = self.search_layer(node_dist, &[ep], ef.max(top), 0);
        w.truncate(top);
        w
    }

    /// Beam search at one layer; returns up to `ef` candidates ascending by distance.
    fn search_layer<F: Fn(u32) -> f32>(
        &self,
        node_dist: &F,
        entry_points: &[u32],
        ef: usize,
        lc: usize,
    ) -> Vec<Cand> {
        let mut visited: HashSet<u32> = HashSet::new();
        let mut frontier: BinaryHeap<std::cmp::Reverse<Cand>> = BinaryHeap::new();
        let mut results: BinaryHeap<Cand> = BinaryHeap::new();

        for &e in entry_points {
            let c = Cand {
                dist: node_dist(e),
                id: e,
            };
            visited.insert(e);
            frontier.push(std::cmp::Reverse(c));
            results.push(c);
        }

        while let Some(std::cmp::Reverse(current)) = frontier.pop() {
            let farthest = results.peek().map(|c| c.dist).unwrap_or(f32::INFINITY);
            if current.dist > farthest && results.len() >= ef {
                break;
            }
            for &nb in &self.links[current.id as usize][lc] {
                if visited.insert(nb) {
                    let d = node_dist(nb);
                    let farthest = results.peek().map(|c| c.dist).unwrap_or(f32::INFINITY);
                    if d < farthest || results.len() < ef {
                        let c = Cand { dist: d, id: nb };
                        frontier.push(std::cmp::Reverse(c));
                        results.push(c);
                        if results.len() > ef {
                            results.pop();
                        }
                    }
                }
            }
        }

        results.into_sorted_vec()
    }
}

/// Neighbor selection heuristic (Malkov Algorithm 4): keep a candidate only if it is
/// closer to the base than to any already-selected neighbor. `candidates` must be sorted
/// ascending by distance to the base (their `dist` field). Falls back to nearest-remaining
/// to reach `m` for connectivity.
fn select_neighbors_heuristic(
    metric: Metric,
    vectors: &[Vec<f32>],
    candidates: &[Cand],
    m: usize,
) -> Vec<u32> {
    let mut selected: Vec<u32> = Vec::with_capacity(m);
    for c in candidates {
        if selected.len() >= m {
            break;
        }
        let mut keep = true;
        for &r in &selected {
            if distance(metric, &vectors[c.id as usize], &vectors[r as usize]) < c.dist {
                keep = false;
                break;
            }
        }
        if keep {
            selected.push(c.id);
        }
    }
    if selected.len() < m {
        for c in candidates {
            if selected.len() >= m {
                break;
            }
            if !selected.contains(&c.id) {
                selected.push(c.id);
            }
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_search_returns_nothing() {
        let h = Hnsw::new(4, HnswParams::default());
        assert!(h.is_empty());
        assert!(h.search(&[0.0, 0.0, 0.0, 0.0], 5, 10).is_empty());
    }

    #[test]
    fn single_node_search_returns_it() {
        let mut h = Hnsw::new(3, HnswParams::default());
        let id = h.insert(vec![1.0, 2.0, 3.0]);
        let res = h.search(&[1.0, 2.0, 3.0], 1, 10);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, id);
        assert!(res[0].1.abs() < 1e-6);
    }

    #[test]
    fn finds_exact_match_among_many() {
        let mut h = Hnsw::new(2, HnswParams::default());
        for i in 0..50 {
            h.insert(vec![i as f32, 0.0]);
        }
        let res = h.search(&[17.0, 0.0], 1, 32);
        assert_eq!(res[0].0, 17);
    }

    #[test]
    fn quantized_search_finds_exact_match() {
        let mut h = Hnsw::new(2, HnswParams::default());
        for i in 0..50 {
            h.insert(vec![i as f32, 0.0]);
        }
        h.quantize();
        assert!(h.is_quantized());
        let res = h.search_quantized(&[17.0, 0.0], 1, 32);
        assert_eq!(res[0].0, 17);
    }
}
