//! The engine: ties the WAL to the memtable, and flushes the memtable to a VSS file.
//!
//! v0 uses auto-commit (each write is its own committed transaction, fsynced immediately).
//! Recovery replays the WAL, applies only committed transactions, and reopens the log for
//! continued appends. Flush writes the live rows as a `.vss` file.

use std::collections::{BTreeSet, HashSet};
use std::io;
use std::path::Path;

use ll_format::{
    write_file_with, Column, ColumnValues, FileMeta, MvccSummary, RawSection, SectionEncoding,
    SectionType, WriteOptions,
};
use ll_graph::{serialize_edge_index, EdgeIndex};
use ll_index::{encode_recall_curve, serialize_hnsw, Hnsw, HnswParams, RECALL_EF_LADDER};
use ll_text::{serialize_text_index, InvertedIndex};
use ll_wal::{RecordType, Wal};

use crate::codec;
use crate::memtable::{Memtable, Op, Row};
use crate::value::Value;

/// Reserved system column id holding each row's `xmin` (creation LSN) in a flushed file.
pub const SYS_XMIN_COL: u32 = u32::MAX;

/// Flush-time ANN recall probe: recall@`RECALL_PROBE_K` is measured across `RECALL_EF_LADDER`
/// over `RECALL_SAMPLE` sampled queries, and the curve is stored so the planner can pick an
/// `ef` that meets the target (or fall back to exact pre-filter).
const RECALL_PROBE_K: usize = 10;
const RECALL_SAMPLE: usize = 64;
/// Reserved system column id holding each row's `xmax` (deletion LSN, or `u64::MAX` if
/// live). A flushed tombstone has `xmax == xmin`, so it is visible-but-deleted at any
/// snapshot past its creation, suppressing the row's stale copy in older segments.
pub const SYS_XMAX_COL: u32 = u32::MAX - 2;

/// A single-node write engine (WAL + memtable).
pub struct Engine {
    wal: Wal,
    mem: Memtable,
    next_txn: u64,
}

impl Engine {
    /// Create a fresh engine backed by a new WAL at `wal_path`.
    pub fn create<P: AsRef<Path>>(wal_path: P, base_lsn: u64) -> io::Result<Self> {
        Ok(Engine {
            wal: Wal::create(wal_path, base_lsn)?,
            mem: Memtable::new(),
            next_txn: 1,
        })
    }

    /// Insert/replace a row (auto-committed and fsynced). Returns the write LSN.
    pub fn insert(&mut self, row_id: u64, row: Row) -> io::Result<u64> {
        let txn = self.next_txn;
        self.next_txn += 1;
        let lsn = self
            .wal
            .append(txn, RecordType::InsertRow, &codec::encode_put(row_id, &row))?;
        self.wal.commit(txn)?;
        self.mem.apply(row_id, Op::Put(row), lsn);
        Ok(lsn)
    }

    /// Delete a row (auto-committed and fsynced). Returns the write LSN.
    pub fn delete(&mut self, row_id: u64) -> io::Result<u64> {
        let txn = self.next_txn;
        self.next_txn += 1;
        let lsn = self
            .wal
            .append(txn, RecordType::DeleteRow, &codec::encode_delete(row_id))?;
        self.wal.commit(txn)?;
        self.mem.apply(row_id, Op::Delete, lsn);
        Ok(lsn)
    }

    /// Visible row at the latest snapshot.
    pub fn get(&self, row_id: u64) -> Option<&Row> {
        self.mem.get(row_id, self.mem.snapshot_lsn())
    }

    pub fn live_count(&self) -> usize {
        self.mem.live_count()
    }

    pub fn memtable(&self) -> &Memtable {
        &self.mem
    }

    /// Replace the memtable with an empty one (e.g. after flushing it to a file). The WAL
    /// and LSN sequence are unaffected; recovery still rebuilds from the log.
    pub fn reset_memtable(&mut self) {
        self.mem = Memtable::new();
    }

    /// Recover an engine from an existing WAL: replay committed transactions into a fresh
    /// memtable and reopen the log for continued appends.
    pub fn recover<P: AsRef<Path>>(wal_path: P) -> io::Result<Self> {
        Self::recover_after(wal_path, 0)
    }

    /// Recover applying only committed records with `lsn > checkpoint_lsn`. Records at or
    /// below the checkpoint are already durable in flushed segments, so replaying them would
    /// double-apply state the segments already hold. (Plain [`recover`] passes `0`.)
    pub fn recover_after<P: AsRef<Path>>(wal_path: P, checkpoint_lsn: u64) -> io::Result<Self> {
        // Truncate any torn tail and reopen for appending; reuse the replay it returns.
        let (wal, r) = Wal::open_append(&wal_path)?;

        let committed: HashSet<u64> = r
            .records
            .iter()
            .filter(|rec| rec.rtype == RecordType::Commit)
            .map(|rec| rec.txn_id)
            .collect();

        let mut mem = Memtable::new();
        let mut max_txn = 0u64;
        for rec in &r.records {
            max_txn = max_txn.max(rec.txn_id);
            if rec.lsn > checkpoint_lsn && committed.contains(&rec.txn_id) {
                if let Some((row_id, op)) = codec::decode(rec.rtype, &rec.payload) {
                    mem.apply(row_id, op, rec.lsn);
                }
            }
        }

        Ok(Engine {
            wal,
            mem,
            next_txn: max_txn + 1,
        })
    }

    /// The next LSN the WAL will assign — everything below it is already written. Used as the
    /// new base after a flush checkpoint.
    pub fn next_lsn(&self) -> u64 {
        self.wal.next_lsn()
    }

    /// Truncate the WAL, restarting LSNs at `base_lsn`, after a flush has made all prior
    /// records durable in a segment. Correctness does not depend on this (recovery filters by
    /// the manifest checkpoint LSN); it just bounds WAL growth.
    pub fn checkpoint_wal(&mut self, base_lsn: u64) -> io::Result<()> {
        self.wal.checkpoint_truncate(base_lsn)
    }

    /// Flush every visible row (live rows **and** tombstones) to a fully-indexed `.vss`
    /// file: column chunks, `xmin`/`xmax` system columns for MVCC, and the embedded HNSW /
    /// text / edge index sections built from the live rows. Persisting tombstones lets a
    /// delete keep suppressing the row's stale copy in older segments. Returns the number
    /// of entries written (live + tombstones).
    pub fn flush_to<P: AsRef<Path>>(&self, vss_path: P) -> io::Result<usize> {
        Self::flush_memtable_to(&self.mem, vss_path)
    }

    /// Flush an arbitrary memtable's visible entries to a fully-indexed `.vss` file. This is
    /// the shared machinery behind [`flush_to`] (this engine's own memtable). Compaction
    /// builds a synthetic memtable holding the merged newest-live version of every row across
    /// the old segments + memtable and flushes it through this same path, so the rewritten
    /// segment is indexed identically to a normal flush.
    pub fn flush_memtable_to<P: AsRef<Path>>(mem: &Memtable, vss_path: P) -> io::Result<usize> {
        let visible = mem.visible_entries_versioned();
        let entries: Vec<(u64, Option<&Row>)> = visible.iter().map(|(id, _, r)| (*id, *r)).collect();
        let n = entries.len();
        let translation: Vec<u64> = visible.iter().map(|(id, _, _)| *id).collect();
        let live_count = visible.iter().filter(|(_, _, r)| r.is_some()).count() as u64;
        let tombstone_count = n as u64 - live_count;

        let mut columns = build_columns(&entries);
        // xmin: creation LSN per entry. xmax: deletion LSN for tombstones (xmax == xmin),
        // u64::MAX for live rows. Both aligned to local offset / entry order.
        columns.push(Column {
            column_id: SYS_XMIN_COL,
            is_system: true,
            values: ColumnValues::I64(visible.iter().map(|(_, x, _)| Some(*x as i64)).collect()),
        });
        columns.push(Column {
            column_id: SYS_XMAX_COL,
            is_system: true,
            values: ColumnValues::I64(
                visible
                    .iter()
                    .map(|(_, x, r)| Some(if r.is_some() { u64::MAX as i64 } else { *x as i64 }))
                    .collect(),
            ),
        });

        let extra = build_index_sections(&entries);

        let meta = FileMeta {
            file_uuid: [0u8; 16],
            min_lsn: mem.min_lsn(),
            max_lsn: mem.max_lsn(),
            row_count: n as u64,
            schema_fingerprint: 0,
            creation_unix_nanos: 0,
            schema_blob: Vec::new(),
            mvcc: MvccSummary {
                min_xmin: mem.min_lsn(),
                max_xmax: u64::MAX,
                tombstone_count,
                live_count,
            },
            writer_version: "ll-engine".into(),
        };
        write_file_with(vss_path, &meta, &columns, &translation, &extra, &WriteOptions::default())
            .map_err(io::Error::other)?;
        Ok(n)
    }
}

/// Build the embedded index sections (HNSW / Text / Edge) from the live entries' columns.
/// Each node/doc is identified by its local offset (entry order, including tombstone slots);
/// tombstones contribute no vector/doc/edges. Edge targets are global ids.
fn build_index_sections(entries: &[(u64, Option<&Row>)]) -> Vec<RawSection> {
    let n = entries.len();
    let cids: BTreeSet<u32> = entries
        .iter()
        .flat_map(|(_, r)| r.iter().flat_map(|row| row.keys().copied()))
        .collect();
    let mut extra = Vec::new();
    for cid in cids {
        match first_value(entries, cid) {
            Some(Value::Vector(first)) => {
                let dim = first.len();
                let mut idx = Hnsw::new(dim, HnswParams::default());
                let mut local_offsets = Vec::new();
                for (local, (_, row)) in entries.iter().enumerate() {
                    if let Some(Value::Vector(v)) = row.and_then(|r| r.get(&cid)) {
                        if v.len() == dim {
                            idx.insert(v.clone());
                            local_offsets.push(local as u32);
                        }
                    }
                }
                idx.quantize();
                // Measure the index's unfiltered recall@k across the ef ladder so the planner
                // can pick an ef that meets the target — or fall back to exact pre-filter.
                let curve = idx.measure_recall_curve(RECALL_PROBE_K, RECALL_EF_LADDER, RECALL_SAMPLE);
                extra.push(section(SectionType::Hnsw, cid, serialize_hnsw(&idx, cid, &local_offsets)));
                extra.push(section(SectionType::OptimizerStats, cid, encode_recall_curve(&curve)));
            }
            Some(Value::Utf8(_)) => {
                let docs: Vec<Option<&str>> = entries
                    .iter()
                    .map(|(_, r)| match r.and_then(|row| row.get(&cid)) {
                        Some(Value::Utf8(s)) => Some(s.as_str()),
                        _ => None,
                    })
                    .collect();
                let idx = InvertedIndex::build(&docs);
                extra.push(section(SectionType::Text, cid, serialize_text_index(&idx, cid)));
            }
            Some(Value::Edges(_)) => {
                let mut edges = Vec::new();
                for (local, (_, row)) in entries.iter().enumerate() {
                    if let Some(Value::Edges(ts)) = row.and_then(|r| r.get(&cid)) {
                        for &t in ts {
                            edges.push((local as u32, t));
                        }
                    }
                }
                let idx = EdgeIndex::build(&edges, n);
                extra.push(section(SectionType::Edge, cid, serialize_edge_index(&idx, cid)));
            }
            _ => {}
        }
    }
    extra
}

fn section(kind: SectionType, column_id: u32, bytes: Vec<u8>) -> RawSection {
    RawSection {
        kind,
        column_id,
        encoding: SectionEncoding::Plain,
        bytes,
    }
}

fn first_value<'a>(entries: &'a [(u64, Option<&Row>)], cid: u32) -> Option<&'a Value> {
    entries.iter().find_map(|(_, r)| match r.and_then(|row| row.get(&cid)) {
        Some(Value::Null) | None => None,
        Some(v) => Some(v),
    })
}

/// Inferred column type for flush.
#[derive(Clone, Copy)]
enum ColKind {
    Bool,
    I64,
    F64,
    Utf8,
    Vector(u16),
}

fn infer_kind(entries: &[(u64, Option<&Row>)], cid: u32) -> Option<ColKind> {
    for (_, r) in entries {
        match r.and_then(|row| row.get(&cid)) {
            Some(Value::Bool(_)) => return Some(ColKind::Bool),
            Some(Value::I64(_)) => return Some(ColKind::I64),
            Some(Value::F64(_)) => return Some(ColKind::F64),
            Some(Value::Utf8(_)) => return Some(ColKind::Utf8),
            Some(Value::Vector(v)) => return Some(ColKind::Vector(v.len() as u16)),
            _ => continue,
        }
    }
    None
}

/// Convert the flush entries into ll-format columns (entry order == local offset). A
/// tombstone entry (`None` row) contributes a null in every data column.
fn build_columns(entries: &[(u64, Option<&Row>)]) -> Vec<Column> {
    let cids: BTreeSet<u32> = entries
        .iter()
        .flat_map(|(_, r)| r.iter().flat_map(|row| row.keys().copied()))
        .collect();
    let mut columns = Vec::with_capacity(cids.len());
    for cid in cids {
        // Skip columns we can't type as a scalar/vector column (edge columns and
        // all-null columns); edges are flushed as Edge sections in a later milestone.
        let Some(kind) = infer_kind(entries, cid) else {
            continue;
        };
        fn cell(r: Option<&Row>, cid: u32) -> Option<&Value> {
            r.and_then(|row| row.get(&cid))
        }
        let values = match kind {
            ColKind::Bool => ColumnValues::Bool(
                entries
                    .iter()
                    .map(|(_, r)| match cell(*r, cid) {
                        Some(Value::Bool(b)) => Some(*b),
                        _ => None,
                    })
                    .collect(),
            ),
            ColKind::I64 => ColumnValues::I64(
                entries
                    .iter()
                    .map(|(_, r)| match cell(*r, cid) {
                        Some(Value::I64(x)) => Some(*x),
                        _ => None,
                    })
                    .collect(),
            ),
            ColKind::F64 => ColumnValues::F64(
                entries
                    .iter()
                    .map(|(_, r)| match cell(*r, cid) {
                        Some(Value::F64(x)) => Some(*x),
                        _ => None,
                    })
                    .collect(),
            ),
            ColKind::Utf8 => ColumnValues::Utf8(
                entries
                    .iter()
                    .map(|(_, r)| match cell(*r, cid) {
                        Some(Value::Utf8(s)) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            ColKind::Vector(dim) => ColumnValues::Vector {
                dim,
                data: entries
                    .iter()
                    .map(|(_, r)| match cell(*r, cid) {
                        Some(Value::Vector(v)) => Some(v.clone()),
                        _ => None,
                    })
                    .collect(),
            },
        };
        columns.push(Column {
            column_id: cid,
            is_system: false,
            values,
        });
    }
    columns
}
