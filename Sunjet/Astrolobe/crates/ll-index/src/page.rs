//! On-disk HNSW section: serialize an [`Hnsw`] to bytes (with BFS reordering) and search
//! over the serialized form without rebuilding the graph. See byte-layout §3.1.
//!
//! Identity model (D-006): serialized node ids are `hnsw_node_id` in BFS order; each slot
//! stores the row's `local_offset` so callers can fetch the full-precision vector from the
//! column chunk for rerank. Neighbor lists are `hnsw_node_id`s.
//!
//! v0 simplifications: slots are packed flat (slot `i` at `pages_off + i*slot_size`),
//! which preserves O(1) lookup; the 4096-byte page grouping is a disk-IO optimization
//! deferred with the buffer pool. There is no internal section CRC — when embedded in an
//! `.vss` file the framing layer's `SectionEntry.crc32` covers these bytes.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};

use crate::hnsw::Hnsw;
use crate::quant::ScalarQuantizer;

/// Errors from parsing a serialized HNSW section.
#[derive(Debug)]
pub enum IndexError {
    Truncated(&'static str),
    Bad(String),
}

/// One search result: a graph node, the row it points at, and its (quantized) distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neighbor {
    pub hnsw_node_id: u32,
    pub local_offset: u32,
    pub quantized_dist: f32,
}

const HEADER_LEN: usize = 66;
const QUANT_FLAG_8BIT: u8 = 1;

// ---- serialization ----------------------------------------------------------------

/// Serialize an HNSW index to section bytes, applying BFS reordering. `local_offsets`
/// maps each in-memory node id (insertion order) to its row `local_offset` in the file.
///
/// # Panics
/// If the index has not been quantized, or `local_offsets.len() != index.len()`.
pub fn serialize_hnsw(index: &Hnsw, column_id: u32, local_offsets: &[u32]) -> Vec<u8> {
    let quant = index
        .quantizer()
        .expect("call Hnsw::quantize() before serializing");
    let n = index.len();
    assert_eq!(local_offsets.len(), n, "local_offsets must cover every node");

    let dim = index.dim();
    let m = index.m();
    let m0 = index.m0();
    let max_level = index.max_level();

    // BFS reorder: new_to_old[new_id] = old_id; old_to_new is the inverse.
    let new_to_old = bfs_order(index);
    let mut old_to_new = vec![0u32; n.max(1)];
    for (new_id, &old) in new_to_old.iter().enumerate() {
        old_to_new[old as usize] = new_id as u32;
    }
    let entry_point = index
        .entry()
        .map(|e| old_to_new[e as usize])
        .unwrap_or(0);

    let slot_size = slot_size(m0, dim);
    let nodes_per_page = (4096 / slot_size).max(1);
    let num_pages = n.div_ceil(nodes_per_page);

    // --- slot region (flat) ---
    let mut slots = vec![0u8; n * slot_size];
    for (new_id, &old_id) in new_to_old.iter().enumerate() {
        let old = old_id as usize;
        let base = new_id * slot_size;
        let s = &mut slots[base..base + slot_size];
        s[0..4].copy_from_slice(&local_offsets[old].to_le_bytes());
        let level = index.node_level(old as u32);
        s[4] = level.min(255) as u8;
        let l0 = index.neighbors(old as u32, 0);
        s[5] = l0.len().min(m0) as u8;
        // s[6..8] pad stays zero
        let nbr_base = 8;
        for i in 0..m0 {
            let val = if i < l0.len() {
                old_to_new[l0[i] as usize]
            } else {
                u32::MAX
            };
            let off = nbr_base + i * 4;
            s[off..off + 4].copy_from_slice(&val.to_le_bytes());
        }
        let qoff = nbr_base + m0 * 4;
        s[qoff..qoff + dim].copy_from_slice(index.code(old as u32));
    }

    // --- upper-layer blob ---
    let upper: Vec<usize> = (0..n)
        .filter(|&new_id| index.node_level(new_to_old[new_id]) > 0)
        .collect();
    let mut records: Vec<u8> = Vec::new();
    let mut idx: Vec<(u32, u32)> = Vec::with_capacity(upper.len());
    for &new_id in &upper {
        let old = new_to_old[new_id] as u32;
        let level = index.node_level(old);
        idx.push((new_id as u32, records.len() as u32));
        records.push(level.min(255) as u8);
        for layer in 1..=level {
            let nbrs = index.neighbors(old, layer);
            records.push(nbrs.len().min(255) as u8);
            for &nb in nbrs {
                records.extend_from_slice(&old_to_new[nb as usize].to_le_bytes());
            }
        }
    }
    let mut upper_blob: Vec<u8> = Vec::new();
    upper_blob.extend_from_slice(&(idx.len() as u32).to_le_bytes());
    for (id, off) in &idx {
        upper_blob.extend_from_slice(&id.to_le_bytes());
        upper_blob.extend_from_slice(&off.to_le_bytes());
    }
    upper_blob.extend_from_slice(&records);

    // --- assemble: header | quant block | slots | upper blob ---
    let quant_off = HEADER_LEN as u64;
    let quant_len = (dim * 8) as u64; // min[dim] f32 + scale[dim] f32
    let pages_off = quant_off + quant_len;
    let upper_off = pages_off + slots.len() as u64;
    let upper_len = upper_blob.len() as u64;

    let mut out = Vec::with_capacity(HEADER_LEN + quant_len as usize + slots.len() + upper_blob.len());
    out.extend_from_slice(&column_id.to_le_bytes());
    out.extend_from_slice(&(dim as u16).to_le_bytes());
    out.extend_from_slice(&(m as u16).to_le_bytes());
    out.extend_from_slice(&(m0 as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // ef_construction (diagnostic; unused at read)
    out.push(QUANT_FLAG_8BIT);
    out.push((max_level + 1).min(255) as u8); // num_levels
    out.extend_from_slice(&entry_point.to_le_bytes());
    out.extend_from_slice(&(nodes_per_page as u32).to_le_bytes());
    out.extend_from_slice(&(num_pages as u32).to_le_bytes());
    out.extend_from_slice(&(n as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // recall_milli (unknown)
    out.extend_from_slice(&quant_off.to_le_bytes());
    out.extend_from_slice(&pages_off.to_le_bytes());
    out.extend_from_slice(&upper_off.to_le_bytes());
    out.extend_from_slice(&upper_len.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);

    for &x in &quant.min {
        out.extend_from_slice(&x.to_le_bytes());
    }
    for &x in &quant.scale {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.extend_from_slice(&slots);
    out.extend_from_slice(&upper_blob);
    out
}

fn slot_size(m0: usize, dim: usize) -> usize {
    8 + m0 * 4 + dim
}

/// BFS order over the layer-0 graph starting from the highest-degree node; returns
/// `new_to_old`. Unreached nodes (if any) are appended in id order.
fn bfs_order(index: &Hnsw) -> Vec<u32> {
    let n = index.len();
    if n == 0 {
        return Vec::new();
    }
    let mut start = 0usize;
    let mut best = 0usize;
    for i in 0..n {
        let d = index.neighbors(i as u32, 0).len();
        if d > best {
            best = d;
            start = i;
        }
    }
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut queue = VecDeque::new();
    queue.push_back(start);
    visited[start] = true;
    while let Some(u) = queue.pop_front() {
        order.push(u as u32);
        for &nb in index.neighbors(u as u32, 0) {
            if !visited[nb as usize] {
                visited[nb as usize] = true;
                queue.push_back(nb as usize);
            }
        }
    }
    for (i, &v) in visited.iter().enumerate() {
        if !v {
            order.push(i as u32);
        }
    }
    order
}

// ---- view + search ----------------------------------------------------------------

/// A read-only view over serialized HNSW section bytes.
pub struct HnswView<'a> {
    bytes: &'a [u8],
    dim: usize,
    m0: usize,
    num_levels: usize,
    entry_point: u32,
    num_nodes: usize,
    slot_size: usize,
    pages_off: usize,
    upper_off: usize,
    quant: ScalarQuantizer,
}

impl<'a> HnswView<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, IndexError> {
        if bytes.len() < HEADER_LEN {
            return Err(IndexError::Truncated("header"));
        }
        // Header field offsets (must match the writer order in `serialize_hnsw`):
        // column_id@0 dim@4 m@6 m0@8 ef@10 quant@12 num_levels@13 entry@14
        // nodes_per_page@18 num_pages@22 num_nodes@26 recall@30
        // quant_off@34 pages_off@42 upper_off@50 upper_len@58
        let dim = rd_u16(bytes, 4) as usize;
        let m0 = rd_u16(bytes, 8) as usize;
        let num_levels = bytes[13] as usize;
        let entry_point = rd_u32(bytes, 14);
        let num_nodes = rd_u32(bytes, 26) as usize;
        let quant_off = rd_u64(bytes, 34) as usize;
        let pages_off = rd_u64(bytes, 42) as usize;
        let upper_off = rd_u64(bytes, 50) as usize;

        // Parse quant block: min[dim], scale[dim] (checked arithmetic — offsets are from
        // untrusted bytes and could be enormous).
        let quant_bytes = dim.checked_mul(8).ok_or(IndexError::Truncated("quant size"))?;
        let quant_end = quant_off
            .checked_add(quant_bytes)
            .ok_or(IndexError::Truncated("quant off"))?;
        if quant_end > bytes.len() {
            return Err(IndexError::Truncated("quant block"));
        }
        let qmin_off = quant_off;
        let qscale_off = quant_off + dim * 4;
        let min: Vec<f32> = (0..dim).map(|d| rd_f32(bytes, qmin_off + d * 4)).collect();
        let scale: Vec<f32> = (0..dim).map(|d| rd_f32(bytes, qscale_off + d * 4)).collect();

        let slot_size = slot_size(m0, dim);
        let slots_bytes = num_nodes
            .checked_mul(slot_size)
            .ok_or(IndexError::Truncated("slots size"))?;
        let slots_end = pages_off
            .checked_add(slots_bytes)
            .ok_or(IndexError::Truncated("slots off"))?;
        if slots_end > bytes.len() {
            return Err(IndexError::Truncated("slots"));
        }
        if upper_off > bytes.len() {
            return Err(IndexError::Truncated("upper blob off"));
        }
        Ok(HnswView {
            bytes,
            dim,
            m0,
            num_levels,
            entry_point,
            num_nodes,
            slot_size,
            pages_off,
            upper_off,
            quant: ScalarQuantizer { min, scale },
        })
    }

    pub fn len(&self) -> usize {
        self.num_nodes
    }
    pub fn is_empty(&self) -> bool {
        self.num_nodes == 0
    }
    pub fn dim(&self) -> usize {
        self.dim
    }

    fn slot(&self, id: u32) -> Option<&[u8]> {
        let base = self
            .pages_off
            .checked_add((id as usize).checked_mul(self.slot_size)?)?;
        let end = base.checked_add(self.slot_size)?;
        self.bytes.get(base..end)
    }

    fn local_offset(&self, id: u32) -> Option<u32> {
        self.slot(id).map(|s| rd_u32(s, 0))
    }

    fn code(&self, id: u32) -> Option<&[u8]> {
        let s = self.slot(id)?;
        let qoff = 8 + self.m0 * 4;
        s.get(qoff..qoff + self.dim)
    }

    fn neighbors(&self, id: u32, layer: usize, out: &mut Vec<u32>) {
        out.clear();
        if layer == 0 {
            let Some(s) = self.slot(id) else { return };
            let count = (s.get(5).copied().unwrap_or(0) as usize).min(self.m0);
            for i in 0..count {
                let Some(v) = g32(s, 8 + i * 4) else { break };
                out.push(v);
            }
            return;
        }
        // upper blob: count u32, then index[(id u32, off u32)], then records.
        let Some(blob) = self.bytes.get(self.upper_off..) else { return };
        let count = match g32(blob, 0) {
            Some(c) => c as usize,
            None => return,
        };
        let index_base = 4usize;
        let records_base = match count.checked_mul(8).and_then(|x| index_base.checked_add(x)) {
            Some(rb) if rb <= blob.len() => rb,
            _ => return,
        };
        // binary search for id in the sorted index
        let (mut lo, mut hi) = (0usize, count);
        let mut rec_off = None;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let Some(entry_id) = g32(blob, index_base + mid * 8) else { return };
            match entry_id.cmp(&id) {
                Ordering::Equal => {
                    rec_off = g32(blob, index_base + mid * 8 + 4).map(|x| x as usize);
                    break;
                }
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
            }
        }
        let Some(rec_off) = rec_off else { return };
        let mut p = match records_base.checked_add(rec_off) {
            Some(p) => p,
            None => return,
        };
        let level = match blob.get(p) {
            Some(&l) => l as usize,
            None => return,
        };
        p += 1;
        // skip layers below `layer`
        for l in 1..=level {
            let cnt = match blob.get(p) {
                Some(&c) => c as usize,
                None => return,
            };
            p += 1;
            if l == layer {
                for i in 0..cnt {
                    let Some(v) = g32(blob, p + i * 4) else { return };
                    out.push(v);
                }
                return;
            }
            p = match p.checked_add(cnt.saturating_mul(4)) {
                Some(p) => p,
                None => return,
            };
        }
    }

    /// Quantized-traversal search; the caller reranks via `local_offset`.
    pub fn search(&self, query: &[f32], ef: usize) -> Vec<Neighbor> {
        self.search_inner(query, ef, |_| true).0
    }

    /// Like [`HnswView::search`] but also returns the number of distance evaluations
    /// performed — a hardware-independent cost proxy for the filtered-search benchmark.
    pub fn search_counted(&self, query: &[f32], ef: usize) -> (Vec<Neighbor>, usize) {
        self.search_inner(query, ef, |_| true)
    }

    /// Integrated-filter search: traversal explores the graph for connectivity, but only
    /// rows satisfying `admit(local_offset)` enter the result set. Returns the candidates
    /// (all satisfying the predicate) and the distance-evaluation count.
    pub fn search_filtered(
        &self,
        query: &[f32],
        ef: usize,
        admit: impl Fn(u32) -> bool,
    ) -> (Vec<Neighbor>, usize) {
        self.search_inner(query, ef, admit)
    }

    fn search_inner<A: Fn(u32) -> bool>(
        &self,
        query: &[f32],
        ef: usize,
        admit: A,
    ) -> (Vec<Neighbor>, usize) {
        if self.num_nodes == 0 {
            return (Vec::new(), 0);
        }
        let qq = self.quant.quantize(query);
        let evals = std::cell::Cell::new(0usize);
        let node_dist = |id: u32| {
            evals.set(evals.get() + 1);
            match self.code(id) {
                Some(c) if c.len() == qq.len() => self.quant.l2_quantized(&qq, c),
                _ => f32::INFINITY,
            }
        };
        let mut scratch = Vec::new();

        let mut ep = self.entry_point;
        let max_level = self.num_levels.saturating_sub(1);
        for lc in (1..=max_level).rev() {
            let w = self.search_layer(&node_dist, &[ep], 1, lc, &mut scratch);
            if let Some(best) = w.first() {
                ep = best.id;
            }
        }
        let w = self.beam_layer0_gated(&node_dist, &[ep], ef, &admit, &mut scratch);
        let out = w
            .into_iter()
            .filter_map(|c| {
                self.local_offset(c.id).map(|lo| Neighbor {
                    hnsw_node_id: c.id,
                    local_offset: lo,
                    quantized_dist: c.dist,
                })
            })
            .collect();
        (out, evals.get())
    }

    /// Layer-0 beam where every visited node is traversed (for connectivity) but only
    /// `admit`-passing nodes enter the result heap. With `admit = |_| true` this is the
    /// ordinary unfiltered beam.
    fn beam_layer0_gated<F: Fn(u32) -> f32, A: Fn(u32) -> bool>(
        &self,
        node_dist: &F,
        entry_points: &[u32],
        ef: usize,
        admit: &A,
        scratch: &mut Vec<u32>,
    ) -> Vec<VCand> {
        let mut visited: HashSet<u32> = HashSet::new();
        let mut frontier: BinaryHeap<std::cmp::Reverse<VCand>> = BinaryHeap::new();
        let mut results: BinaryHeap<VCand> = BinaryHeap::new();

        for &e in entry_points {
            let c = VCand {
                dist: node_dist(e),
                id: e,
            };
            visited.insert(e);
            frontier.push(std::cmp::Reverse(c));
            if self.local_offset(e).map(&admit).unwrap_or(false) {
                results.push(c);
                if results.len() > ef {
                    results.pop();
                }
            }
        }

        while let Some(std::cmp::Reverse(current)) = frontier.pop() {
            if results.len() >= ef {
                if let Some(top) = results.peek() {
                    if current.dist > top.dist {
                        break;
                    }
                }
            }
            self.neighbors(current.id, 0, scratch);
            let neighbors: Vec<u32> = scratch.clone();
            for nb in neighbors {
                if visited.insert(nb) {
                    let d = node_dist(nb);
                    let farthest = if results.len() >= ef {
                        results.peek().map(|c| c.dist).unwrap_or(f32::INFINITY)
                    } else {
                        f32::INFINITY
                    };
                    if d < farthest || results.len() < ef {
                        frontier.push(std::cmp::Reverse(VCand { dist: d, id: nb }));
                    }
                    if self.local_offset(nb).map(&admit).unwrap_or(false) {
                        let c = VCand { dist: d, id: nb };
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

    fn search_layer<F: Fn(u32) -> f32>(
        &self,
        node_dist: &F,
        entry_points: &[u32],
        ef: usize,
        lc: usize,
        scratch: &mut Vec<u32>,
    ) -> Vec<VCand> {
        let mut visited: HashSet<u32> = HashSet::new();
        let mut frontier: BinaryHeap<std::cmp::Reverse<VCand>> = BinaryHeap::new();
        let mut results: BinaryHeap<VCand> = BinaryHeap::new();

        for &e in entry_points {
            let c = VCand {
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
            self.neighbors(current.id, lc, scratch);
            let neighbors: Vec<u32> = scratch.clone();
            for nb in neighbors {
                if visited.insert(nb) {
                    let d = node_dist(nb);
                    let farthest = results.peek().map(|c| c.dist).unwrap_or(f32::INFINITY);
                    if d < farthest || results.len() < ef {
                        let c = VCand { dist: d, id: nb };
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

#[derive(Debug, Clone, Copy)]
struct VCand {
    dist: f32,
    id: u32,
}
impl PartialEq for VCand {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id && self.dist.to_bits() == o.dist.to_bits()
    }
}
impl Eq for VCand {}
impl Ord for VCand {
    fn cmp(&self, o: &Self) -> Ordering {
        self.dist.total_cmp(&o.dist).then(self.id.cmp(&o.id))
    }
}
impl PartialOrd for VCand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

// Bounds-checked little-endian reads: never panic; return 0 / None on out-of-bounds.
fn rd_u16(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map(|s| u16::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn rd_u32(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
fn rd_f32(b: &[u8], o: usize) -> f32 {
    b.get(o..o + 4).map(|s| f32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0.0)
}
fn g32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
