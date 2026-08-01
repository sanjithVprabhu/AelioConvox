//! In-memory graph index: forward + reverse CSR adjacency with a bloom filter over
//! targets, plus depth-limited forward traversal.
//!
//! Edges are `(source_local_offset: u32, target_global_row_id: u64)` (D-001: targets are
//! global so they survive compaction and cross files). Forward traversal answers "what
//! does S point to?"; reverse answers "what points to T?".

use std::collections::BTreeSet;

use crate::bloom::Bloom;

/// Forward + reverse CSR for one edge column.
#[derive(Debug, Clone)]
pub struct EdgeIndex {
    num_nodes: usize,
    /// `fwd_targets[fwd_offsets[s]..fwd_offsets[s+1]]` = targets of source `s` (sorted).
    fwd_offsets: Vec<u32>,
    fwd_targets: Vec<u64>,
    /// distinct targets, sorted (for reverse binary search).
    rev_targets: Vec<u64>,
    rev_offsets: Vec<u32>,
    rev_sources: Vec<u32>,
    bloom: Bloom,
}

impl EdgeIndex {
    /// Build from `(source_local, target_global)` edges over `num_nodes` source slots.
    pub fn build(edges: &[(u32, u64)], num_nodes: usize) -> Self {
        // Forward: sort by (source, target).
        let mut by_src = edges.to_vec();
        by_src.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut fwd_offsets = vec![0u32; num_nodes + 1];
        for &(s, _) in &by_src {
            fwd_offsets[s as usize + 1] += 1;
        }
        for i in 0..num_nodes {
            fwd_offsets[i + 1] += fwd_offsets[i];
        }
        let fwd_targets: Vec<u64> = by_src.iter().map(|&(_, t)| t).collect();

        // Reverse: sort by (target, source), group by target.
        let mut by_tgt = edges.to_vec();
        by_tgt.sort_unstable_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        let mut rev_targets = Vec::new();
        let mut rev_offsets = vec![0u32];
        let mut rev_sources = Vec::new();
        let mut i = 0;
        while i < by_tgt.len() {
            let t = by_tgt[i].1;
            rev_targets.push(t);
            let mut j = i;
            while j < by_tgt.len() && by_tgt[j].1 == t {
                rev_sources.push(by_tgt[j].0);
                j += 1;
            }
            rev_offsets.push(rev_sources.len() as u32);
            i = j;
        }

        let bloom = Bloom::build(rev_targets.iter().copied(), rev_targets.len());
        EdgeIndex {
            num_nodes,
            fwd_offsets,
            fwd_targets,
            rev_targets,
            rev_offsets,
            rev_sources,
            bloom,
        }
    }

    pub fn num_nodes(&self) -> usize {
        self.num_nodes
    }
    pub fn num_edges(&self) -> usize {
        self.fwd_targets.len()
    }
    pub fn num_targets(&self) -> usize {
        self.rev_targets.len()
    }
    pub(crate) fn fwd_offsets(&self) -> &[u32] {
        &self.fwd_offsets
    }
    pub(crate) fn fwd_targets(&self) -> &[u64] {
        &self.fwd_targets
    }
    pub(crate) fn rev_targets(&self) -> &[u64] {
        &self.rev_targets
    }
    pub(crate) fn rev_offsets(&self) -> &[u32] {
        &self.rev_offsets
    }
    pub(crate) fn rev_sources(&self) -> &[u32] {
        &self.rev_sources
    }
    pub(crate) fn bloom(&self) -> &Bloom {
        &self.bloom
    }

    /// Targets of source `s` ("what does S point to?").
    pub fn out_neighbors(&self, s: u32) -> &[u64] {
        if s as usize >= self.num_nodes {
            return &[];
        }
        let a = self.fwd_offsets[s as usize] as usize;
        let b = self.fwd_offsets[s as usize + 1] as usize;
        &self.fwd_targets[a..b]
    }

    pub fn out_degree(&self, s: u32) -> usize {
        self.out_neighbors(s).len()
    }

    /// Sources pointing at target `t` ("what points to T?").
    pub fn in_neighbors(&self, t: u64) -> &[u32] {
        match self.rev_targets.binary_search(&t) {
            Ok(i) => {
                let a = self.rev_offsets[i] as usize;
                let b = self.rev_offsets[i + 1] as usize;
                &self.rev_sources[a..b]
            }
            Err(_) => &[],
        }
    }

    /// Cross-file pruning hint: may `t` be a target in this file?
    pub fn might_target(&self, t: u64) -> bool {
        self.bloom.might_contain(t)
    }

    /// Depth-limited forward BFS from `seeds` (local ids). `resolve` maps a target global
    /// id back to a local id so traversal can continue within this file; returns `None`
    /// for targets outside it. Returns all reached target global ids.
    pub fn traverse_forward(
        &self,
        seeds: &[u32],
        max_depth: usize,
        resolve: impl Fn(u64) -> Option<u32>,
    ) -> BTreeSet<u64> {
        let mut visited: BTreeSet<u32> = seeds.iter().copied().collect();
        let mut reached: BTreeSet<u64> = BTreeSet::new();
        let mut frontier: Vec<u32> = seeds.to_vec();
        for _ in 0..max_depth {
            let mut next = Vec::new();
            for &node in &frontier {
                for &t in self.out_neighbors(node) {
                    reached.insert(t);
                    if let Some(loc) = resolve(t) {
                        if visited.insert(loc) {
                            next.push(loc);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        reached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny DAG: 0->1, 0->2, 1->3, 2->3, 3->4. Targets are global == local here.
    fn graph() -> EdgeIndex {
        let edges = vec![(0u32, 1u64), (0, 2), (1, 3), (2, 3), (3, 4)];
        EdgeIndex::build(&edges, 5)
    }

    #[test]
    fn forward_and_reverse_adjacency() {
        let g = graph();
        assert_eq!(g.out_neighbors(0), &[1, 2]);
        assert_eq!(g.out_neighbors(3), &[4]);
        assert_eq!(g.out_neighbors(4), &[] as &[u64]);
        assert_eq!(g.in_neighbors(3), &[1, 2]);
        assert_eq!(g.in_neighbors(0), &[] as &[u32]);
        assert_eq!(g.num_edges(), 5);
    }

    #[test]
    fn bloom_has_no_false_negatives() {
        let g = graph();
        for t in [1u64, 2, 3, 4] {
            assert!(g.might_target(t));
        }
    }

    #[test]
    fn depth_limited_traversal() {
        let g = graph();
        let resolve = |t: u64| Some(t as u32); // identity within this file
                                               // 1 hop from 0: {1,2}
        assert_eq!(g.traverse_forward(&[0], 1, resolve), BTreeSet::from([1, 2]));
        // 2 hops from 0: {1,2,3}
        assert_eq!(
            g.traverse_forward(&[0], 2, resolve),
            BTreeSet::from([1, 2, 3])
        );
        // 3 hops reaches 4
        assert_eq!(
            g.traverse_forward(&[0], 3, resolve),
            BTreeSet::from([1, 2, 3, 4])
        );
    }
}
