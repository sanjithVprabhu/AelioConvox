//! On-disk Edge section (byte-layout §3.3): forward + reverse CSR + bloom filter, with an
//! [`EdgeView`] that traverses the bytes without rebuilding. Embeds in a `.vss` file as an
//! opaque `Edge` section.
//!
//! Layout (little-endian, section-relative offsets):
//! ```text
//! header(44B): column_id u32, num_nodes u32, num_edges u32, num_targets u32,
//!              fwd_offsets_off u32, fwd_targets_off u32, rev_targets_off u32,
//!              rev_offsets_off u32, rev_sources_off u32, bloom_off u32, bloom_len u32
//! fwd_offsets: u32[num_nodes+1]      fwd_targets: u64[num_edges]
//! rev_targets: u64[num_targets]      rev_offsets: u32[num_targets+1]
//! rev_sources: u32[num_edges]        bloom: num_bits u32, num_hashes u32, bits[]
//! ```

use std::collections::BTreeSet;

use crate::bloom::Bloom;
use crate::index::EdgeIndex;

const HEADER_LEN: usize = 44;

#[derive(Debug)]
pub enum EdgeError {
    Truncated(&'static str),
}

/// Serialize an edge index to Edge section bytes.
pub fn serialize_edge_index(idx: &EdgeIndex, column_id: u32) -> Vec<u8> {
    let fwd_offsets = idx.fwd_offsets();
    let fwd_targets = idx.fwd_targets();
    let rev_targets = idx.rev_targets();
    let rev_offsets = idx.rev_offsets();
    let rev_sources = idx.rev_sources();
    let bloom = idx.bloom();
    let num_nodes = idx.num_nodes();
    let num_edges = idx.num_edges();
    let num_targets = idx.num_targets();

    let fwd_offsets_off = HEADER_LEN as u32;
    let fwd_targets_off = fwd_offsets_off + (fwd_offsets.len() * 4) as u32;
    let rev_targets_off = fwd_targets_off + (fwd_targets.len() * 8) as u32;
    let rev_offsets_off = rev_targets_off + (rev_targets.len() * 8) as u32;
    let rev_sources_off = rev_offsets_off + (rev_offsets.len() * 4) as u32;
    let bloom_off = rev_sources_off + (rev_sources.len() * 4) as u32;
    let bloom_len = 8 + bloom.bytes().len() as u32; // num_bits + num_hashes + bits

    let mut out = Vec::with_capacity(bloom_off as usize + bloom_len as usize);
    out.extend_from_slice(&column_id.to_le_bytes());
    out.extend_from_slice(&(num_nodes as u32).to_le_bytes());
    out.extend_from_slice(&(num_edges as u32).to_le_bytes());
    out.extend_from_slice(&(num_targets as u32).to_le_bytes());
    out.extend_from_slice(&fwd_offsets_off.to_le_bytes());
    out.extend_from_slice(&fwd_targets_off.to_le_bytes());
    out.extend_from_slice(&rev_targets_off.to_le_bytes());
    out.extend_from_slice(&rev_offsets_off.to_le_bytes());
    out.extend_from_slice(&rev_sources_off.to_le_bytes());
    out.extend_from_slice(&bloom_off.to_le_bytes());
    out.extend_from_slice(&bloom_len.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);

    for &o in fwd_offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    for &t in fwd_targets {
        out.extend_from_slice(&t.to_le_bytes());
    }
    for &t in rev_targets {
        out.extend_from_slice(&t.to_le_bytes());
    }
    for &o in rev_offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    for &s in rev_sources {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out.extend_from_slice(&bloom.num_bits().to_le_bytes());
    out.extend_from_slice(&bloom.num_hashes().to_le_bytes());
    out.extend_from_slice(bloom.bytes());
    out
}

/// A read-only view over serialized Edge section bytes.
pub struct EdgeView<'a> {
    bytes: &'a [u8],
    num_nodes: usize,
    num_targets: usize,
    fwd_offsets_off: usize,
    fwd_targets_off: usize,
    rev_targets_off: usize,
    rev_offsets_off: usize,
    rev_sources_off: usize,
    bloom: Bloom,
}

impl<'a> EdgeView<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, EdgeError> {
        if bytes.len() < HEADER_LEN {
            return Err(EdgeError::Truncated("header"));
        }
        let num_nodes = rd_u32(bytes, 4) as usize;
        let num_targets = rd_u32(bytes, 12) as usize;
        let bloom_off = rd_u32(bytes, 36) as usize;
        let bloom_len = rd_u32(bytes, 40) as usize;
        let bloom_end = bloom_off
            .checked_add(bloom_len)
            .ok_or(EdgeError::Truncated("bloom off"))?;
        if bloom_end > bytes.len() || bloom_len < 8 {
            return Err(EdgeError::Truncated("bloom"));
        }
        let num_bits = rd_u32(bytes, bloom_off);
        let num_hashes = rd_u32(bytes, bloom_off + 4);
        let bloom_bits = bytes.get(bloom_off + 8..bloom_end).unwrap_or(&[]).to_vec();
        let bloom = Bloom::from_parts(bloom_bits, num_bits, num_hashes);

        let view = EdgeView {
            bytes,
            num_nodes,
            num_targets,
            fwd_offsets_off: rd_u32(bytes, 16) as usize,
            fwd_targets_off: rd_u32(bytes, 20) as usize,
            rev_targets_off: rd_u32(bytes, 24) as usize,
            rev_offsets_off: rd_u32(bytes, 28) as usize,
            rev_sources_off: rd_u32(bytes, 32) as usize,
            bloom,
        };
        let len = bytes.len();
        if view.fwd_offsets_off > len
            || view.fwd_targets_off > len
            || view.rev_targets_off > len
            || view.rev_offsets_off > len
            || view.rev_sources_off > len
        {
            return Err(EdgeError::Truncated("regions"));
        }
        Ok(view)
    }

    pub fn num_nodes(&self) -> usize {
        self.num_nodes
    }

    /// Targets of source `s`.
    pub fn out_neighbors(&self, s: u32) -> Vec<u64> {
        if s as usize >= self.num_nodes {
            return Vec::new();
        }
        let a = rd_u32(self.bytes, self.fwd_offsets_off.saturating_add(s as usize * 4)) as usize;
        let b = rd_u32(self.bytes, self.fwd_offsets_off.saturating_add((s as usize + 1) * 4)) as usize;
        // Clamp to the most u64s the buffer could hold (corrupt offsets can't force huge loops).
        let cap = self.bytes.len() / 8 + 1;
        (a..b.min(cap).max(a))
            .map(|i| rd_u64(self.bytes, self.fwd_targets_off.saturating_add(i * 8)))
            .collect()
    }

    /// Sources pointing at target `t`.
    pub fn in_neighbors(&self, t: u64) -> Vec<u32> {
        // binary search rev_targets
        let (mut lo, mut hi) = (0usize, self.num_targets);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let v = rd_u64(self.bytes, self.rev_targets_off.saturating_add(mid * 8));
            match v.cmp(&t) {
                std::cmp::Ordering::Equal => {
                    let a = rd_u32(self.bytes, self.rev_offsets_off.saturating_add(mid * 4)) as usize;
                    let b = rd_u32(self.bytes, self.rev_offsets_off.saturating_add((mid + 1) * 4)) as usize;
                    let cap = self.bytes.len() / 4 + 1;
                    return (a..b.min(cap).max(a))
                        .map(|i| rd_u32(self.bytes, self.rev_sources_off.saturating_add(i * 4)))
                        .collect();
                }
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        Vec::new()
    }

    pub fn might_target(&self, t: u64) -> bool {
        self.bloom.might_contain(t)
    }

    /// Depth-limited forward BFS (see [`EdgeIndex::traverse_forward`]).
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
                for t in self.out_neighbors(node) {
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

// Bounds-checked: never panic; 0 on out-of-bounds.
fn rd_u32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
