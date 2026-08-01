//! `Source` over a flushed `.vss` file: queries the embedded HNSW / text / edge index
//! sections, reranks vectors against the column chunk, and applies MVCC via the `xmin`
//! system column. Returns global row ids, so a file and the memtable are interchangeable
//! to the planner.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aelio_db_engine::{Value, SYS_XMAX_COL, SYS_XMIN_COL};
use aelio_db_format::{ColumnValues, LlFile, SectionType};
use aelio_db_graph::EdgeView;
use aelio_db_index::HnswView;
use aelio_db_text::TextView;

use crate::exec::{PredOp, Predicate};
use crate::source::Source;
use crate::util::{l2_sq, top_k};

/// A read-only `Source` view over one flushed `.vss` file. Owns the file (via `Arc`) and its
/// derived lookup structures, so a `Database` can build it once at load/flush time and keep it
/// resident instead of rebuilding the maps on every query.
pub struct FileSource {
    file: Arc<LlFile>,
    global_to_local: HashMap<u64, u32>,
    /// `xmin` per local offset (creation LSN); 0 if the file has no MVCC column.
    xmin: Vec<u64>,
    /// `xmax` per local offset (deletion LSN; `u64::MAX` = live). A tombstone row has
    /// `xmax == xmin`. Defaults to all-live for legacy files without the column.
    xmax: Vec<u64>,
    /// Per I64 column: `(value, local_offset)` sorted by value — a scalar index for fast
    /// predicate evaluation (built once; the file is immutable).
    scalar_idx: HashMap<u32, Vec<(i64, u32)>>,
}

impl FileSource {
    pub fn new(file: Arc<LlFile>) -> Self {
        let global_to_local = file
            .translation_table
            .iter()
            .enumerate()
            .map(|(local, &global)| (global, local as u32))
            .collect();
        let n = file.translation_table.len();
        let xmin = match file
            .columns
            .iter()
            .find(|c| c.column_id == SYS_XMIN_COL)
            .map(|c| &c.values)
        {
            Some(ColumnValues::I64(v)) => {
                v.iter().map(|x| x.map(|i| i as u64).unwrap_or(0)).collect()
            }
            _ => vec![0u64; n],
        };
        let xmax = match file
            .columns
            .iter()
            .find(|c| c.column_id == SYS_XMAX_COL)
            .map(|c| &c.values)
        {
            Some(ColumnValues::I64(v)) => v
                .iter()
                .map(|x| x.map(|i| i as u64).unwrap_or(u64::MAX))
                .collect(),
            _ => vec![u64::MAX; n],
        };

        // Build a sorted scalar index per (non-system) I64 column.
        let mut scalar_idx = HashMap::new();
        for c in &file.columns {
            if c.column_id == SYS_XMIN_COL || c.column_id == SYS_XMAX_COL {
                continue;
            }
            if let ColumnValues::I64(v) = &c.values {
                let mut pairs: Vec<(i64, u32)> = v
                    .iter()
                    .enumerate()
                    .filter_map(|(i, x)| x.map(|val| (val, i as u32)))
                    .collect();
                pairs.sort_unstable_by_key(|&(val, _)| val);
                scalar_idx.insert(c.column_id, pairs);
            }
        }

        FileSource {
            file,
            global_to_local,
            xmin,
            xmax,
            scalar_idx,
        }
    }

    /// The row existed (was created) as of `snapshot` — regardless of later deletion.
    fn visible(&self, local: u32, snapshot: u64) -> bool {
        (local as usize) < self.xmin.len() && self.xmin[local as usize] <= snapshot
    }

    /// The row is created and not yet deleted as of `snapshot` (a live, data-bearing row).
    fn live(&self, local: u32, snapshot: u64) -> bool {
        (local as usize) < self.xmin.len()
            && self.xmin[local as usize] <= snapshot
            && self.xmax.get(local as usize).copied().unwrap_or(u64::MAX) > snapshot
    }

    fn global(&self, local: u32) -> u64 {
        self.file.translation_table[local as usize]
    }

    fn column(&self, col: u32) -> Option<&ColumnValues> {
        self.file
            .columns
            .iter()
            .find(|c| c.column_id == col)
            .map(|c| &c.values)
    }

    fn vector_at(&self, col: u32, local: u32) -> Option<&[f32]> {
        match self.column(col)? {
            ColumnValues::Vector { data, .. } => data.get(local as usize)?.as_deref(),
            _ => None,
        }
    }
}

impl Source for FileSource {
    fn vector_search(
        &self,
        col: u32,
        query: &[f32],
        k: usize,
        ef: usize,
        snapshot: u64,
    ) -> Vec<(u64, f32)> {
        let Some(bytes) = self.file.section(SectionType::Hnsw, col) else {
            return Vec::new();
        };
        let Ok(view) = HnswView::parse(bytes) else {
            return Vec::new();
        };
        // Breadth has two drivers: the planner's recall-target ef (for base ANN quality), and
        // the selectivity-driven over-fetch (k here is the post-fetch count; filtered queries
        // must search ~8× wider so enough true matches survive the predicate). Use the larger.
        let ef = ef.max(k * 8);
        let mut scored = Vec::new();
        for nb in view.search(query, ef) {
            let local = nb.local_offset;
            if !self.live(local, snapshot) {
                continue;
            }
            if let Some(v) = self.vector_at(col, local) {
                if v.len() == query.len() {
                    scored.push((self.global(local), l2_sq(query, v)));
                }
            }
        }
        top_k(scored, k, true)
    }

    fn text_search(&self, col: u32, query: &str, k: usize, snapshot: u64) -> Vec<(u64, f32)> {
        let Some(bytes) = self.file.section(SectionType::Text, col) else {
            return Vec::new();
        };
        let Ok(view) = TextView::parse(bytes) else {
            return Vec::new();
        };
        let mut scored = Vec::new();
        for (doc, score) in view.bm25(query, k * 4) {
            if self.live(doc, snapshot) {
                scored.push((self.global(doc), score));
            }
        }
        top_k(scored, k, false)
    }

    fn out_neighbors(&self, col: u32, src: u64, snapshot: u64) -> Vec<u64> {
        let Some(&local) = self.global_to_local.get(&src) else {
            return Vec::new();
        };
        if !self.live(local, snapshot) {
            return Vec::new();
        }
        let Some(bytes) = self.file.section(SectionType::Edge, col) else {
            return Vec::new();
        };
        let Ok(view) = EdgeView::parse(bytes) else {
            return Vec::new();
        };
        view.out_neighbors(local)
            .into_iter()
            .filter(|tgt| match self.global_to_local.get(tgt) {
                Some(&tl) => self.live(tl, snapshot), // in this file: drop if deleted here
                None => true, // target lives in another source — pass it through; the
                              // executor resolves its visibility across all sources
            })
            .collect()
    }

    fn in_neighbors(&self, col: u32, tgt: u64, snapshot: u64) -> Vec<u64> {
        let Some(bytes) = self.file.section(SectionType::Edge, col) else {
            return Vec::new();
        };
        let Ok(view) = EdgeView::parse(bytes) else {
            return Vec::new();
        };
        view.in_neighbors(tgt)
            .into_iter()
            .filter(|&local| self.live(local, snapshot))
            .map(|local| self.global(local))
            .collect()
    }

    fn scalar(&self, row_id: u64, col: u32, snapshot: u64) -> Option<Value> {
        let &local = self.global_to_local.get(&row_id)?;
        if !self.live(local, snapshot) {
            return None;
        }
        cell_to_value(self.column(col)?, local as usize)
    }

    fn rows_for_predicates(&self, preds: &[Predicate], snapshot: u64) -> Option<Vec<u64>> {
        if preds.is_empty() {
            return None;
        }
        let mut acc: Option<HashSet<u32>> = None;
        for p in preds {
            let idx = self.scalar_idx.get(&p.col)?; // not indexed → fall back to scan
            let target = match &p.value {
                Value::I64(v) => *v,
                _ => return None, // non-integer predicate → fall back
            };
            let locals = range_locals(idx, p.op, target)?; // unsupported op → fall back
            let set: HashSet<u32> = locals.into_iter().collect();
            acc = Some(match acc {
                None => set,
                Some(prev) => prev.intersection(&set).copied().collect(),
            });
            if acc.as_ref().unwrap().is_empty() {
                return Some(Vec::new());
            }
        }
        let mut out: Vec<u64> = acc
            .unwrap_or_default()
            .into_iter()
            .filter(|&l| self.live(l, snapshot))
            .map(|l| self.global(l))
            .collect();
        out.sort_unstable();
        Some(out)
    }

    fn all_rows(&self, snapshot: u64) -> Vec<u64> {
        (0..self.file.translation_table.len() as u32)
            .filter(|&local| self.live(local, snapshot))
            .map(|local| self.global(local))
            .collect()
    }

    fn sample_rows(&self, snapshot: u64, max: usize) -> (Vec<u64>, usize) {
        let mut sample = Vec::with_capacity(max.min(self.file.translation_table.len()));
        let mut total = 0usize;
        for local in 0..self.file.translation_table.len() as u32 {
            if !self.live(local, snapshot) {
                continue;
            }
            total += 1;
            let id = self.global(local);
            if sample.len() < max {
                sample.push(id);
            } else if max > 0 {
                // Deterministic reservoir sampling keeps allocation bounded while avoiding a
                // first-row bias when row ids correlate with the predicate being estimated.
                let slot = (total.wrapping_mul(0x9E37_79B1) ^ (id as usize)) % total;
                if slot < max {
                    sample[slot] = id;
                }
            }
        }
        (sample, total)
    }

    fn vector(&self, col: u32, row_id: u64, snapshot: u64) -> Option<Vec<f32>> {
        let &local = self.global_to_local.get(&row_id)?;
        if !self.live(local, snapshot) {
            return None;
        }
        self.vector_at(col, local).map(|v| v.to_vec())
    }

    fn version_at(&self, row_id: u64, snapshot: u64) -> Option<(u64, bool)> {
        let &local = self.global_to_local.get(&row_id)?;
        if !self.visible(local, snapshot) {
            return None; // not created as of this snapshot — this segment has no version
        }
        let stamp = self.xmin[local as usize];
        let deleted = self.xmax.get(local as usize).copied().unwrap_or(u64::MAX) <= snapshot;
        Some((stamp, deleted))
    }

    fn ann_recall_curve(&self, col: u32) -> Vec<(usize, f64)> {
        match self.file.section(SectionType::OptimizerStats, col) {
            Some(bytes) => aelio_db_index::decode_recall_curve(bytes),
            None => Vec::new(),
        }
    }
}

/// Local offsets whose value satisfies `op target`, via binary search over the
/// value-sorted index. Returns `None` for `Ne` (not a contiguous range — caller scans).
fn range_locals(idx: &[(i64, u32)], op: PredOp, target: i64) -> Option<Vec<u32>> {
    let lt = idx.partition_point(|&(v, _)| v < target); // first index with v >= target
    let le = idx.partition_point(|&(v, _)| v <= target); // first index with v > target
    let slice = match op {
        PredOp::Lt => &idx[..lt],
        PredOp::Le => &idx[..le],
        PredOp::Gt => &idx[le..],
        PredOp::Ge => &idx[lt..],
        PredOp::Eq => &idx[lt..le],
        PredOp::Ne => return None,
    };
    Some(slice.iter().map(|&(_, local)| local).collect())
}

/// Convert one cell of an `aelio-db-format` column to an `aelio-db-engine` value.
fn cell_to_value(values: &ColumnValues, i: usize) -> Option<Value> {
    match values {
        ColumnValues::Bool(v) => v.get(i).copied().flatten().map(Value::Bool),
        ColumnValues::I32(v) => v.get(i).copied().flatten().map(|x| Value::I64(x as i64)),
        ColumnValues::I64(v) => v.get(i).copied().flatten().map(Value::I64),
        ColumnValues::F32(v) => v.get(i).copied().flatten().map(|x| Value::F64(x as f64)),
        ColumnValues::F64(v) => v.get(i).copied().flatten().map(Value::F64),
        ColumnValues::TimestampNanos(v) => v.get(i).copied().flatten().map(Value::I64),
        ColumnValues::Utf8(v) => v.get(i).cloned().flatten().map(Value::Utf8),
        ColumnValues::Vector { data, .. } => data.get(i).cloned().flatten().map(Value::Vector),
    }
}
