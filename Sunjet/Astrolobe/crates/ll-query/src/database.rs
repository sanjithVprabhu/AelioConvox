//! `Database` — the single-node facade tying the catalog, the engine (WAL + memtable), and
//! the flushed `.vss` segments into one `query()` over named tables and columns.
//!
//! v0: one engine/WAL for the whole database; rows are tagged with a `table_id` system
//! column and queries add an implicit `table_id = T` filter, so multiple tables share the
//! segments. Flush persists the memtable to a new segment and resets it.
//!
//! Durability: [`Database::open`] reconstructs the database from disk — it loads the schema
//! (catalog), the segment list and checkpoint LSN (manifest), and replays the WAL tail past
//! the checkpoint into the memtable. Flush is crash-safe: the segment is fsynced, then the
//! manifest is written atomically (carrying the new checkpoint LSN), then the WAL is
//! truncated. Because recovery filters the WAL by the manifest's checkpoint LSN, a crash at
//! any point never double-applies or loses committed data.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ll_catalog::{Catalog, ColumnKind};
use ll_engine::{Engine, Memtable, Op, Row, Value, WriteOp};
use ll_format::read_file;

use crate::exec::{execute, explain_plan, PredOp, Query};
use crate::file_source::FileSource;
use crate::source::Source;

/// System column holding a row's `table_id` (distinct from `ll_engine::SYS_XMIN_COL`).
const SYS_TABLE_COL: u32 = u32::MAX - 1;
/// Snapshot that sees all committed data (one below the `xmax = ∞` sentinel).
const SNAPSHOT_LATEST: u64 = u64::MAX - 1;

const WAL_NAME: &str = "wal.log";
const CATALOG_NAME: &str = "catalog.bin";
const MANIFEST_NAME: &str = "manifest.bin";
const MANIFEST_MAGIC: u32 = u32::from_le_bytes(*b"LMAN");

/// On-disk database state outside the WAL and segments: id/sequence counters, the LSN up to
/// which data is durable in segments, and the ordered list of segment file names.
#[derive(Debug, Default, Clone)]
struct Manifest {
    next_row_id: u64,
    seg_seq: u64,
    checkpoint_lsn: u64,
    segments: Vec<String>,
}

impl Manifest {
    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&MANIFEST_MAGIC.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // version
        b.extend_from_slice(&self.next_row_id.to_le_bytes());
        b.extend_from_slice(&self.seg_seq.to_le_bytes());
        b.extend_from_slice(&self.checkpoint_lsn.to_le_bytes());
        b.extend_from_slice(&(self.segments.len() as u32).to_le_bytes());
        for name in &self.segments {
            b.extend_from_slice(&(name.len() as u16).to_le_bytes());
            b.extend_from_slice(name.as_bytes());
        }
        b
    }

    fn decode(b: &[u8]) -> Option<Manifest> {
        let mut p = 0usize;
        let take = |p: &mut usize, n: usize| -> Option<&[u8]> {
            let s = b.get(*p..*p + n)?;
            *p += n;
            Some(s)
        };
        if u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) != MANIFEST_MAGIC {
            return None;
        }
        let _version = u16::from_le_bytes(take(&mut p, 2)?.try_into().ok()?);
        let next_row_id = u64::from_le_bytes(take(&mut p, 8)?.try_into().ok()?);
        let seg_seq = u64::from_le_bytes(take(&mut p, 8)?.try_into().ok()?);
        let checkpoint_lsn = u64::from_le_bytes(take(&mut p, 8)?.try_into().ok()?);
        let count = u32::from_le_bytes(take(&mut p, 4)?.try_into().ok()?) as usize;
        let mut segments = Vec::with_capacity(count);
        for _ in 0..count {
            let len = u16::from_le_bytes(take(&mut p, 2)?.try_into().ok()?) as usize;
            segments.push(String::from_utf8(take(&mut p, len)?.to_vec()).ok()?);
        }
        Some(Manifest {
            next_row_id,
            seg_seq,
            checkpoint_lsn,
            segments,
        })
    }
}

/// Write `bytes` to `path` atomically: write a temp file, fsync it, rename over the target.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    // fsync the parent directory so the rename itself survives a crash.
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// A hybrid query expressed over column **names**.
#[derive(Debug, Clone, Default)]
pub struct HybridQuery {
    pub k: usize,
    pub vector: Option<(String, Vec<f32>)>,
    pub text: Option<(String, String)>,
    pub filters: Vec<(String, PredOp, Value)>,
    pub graph: Option<(String, Vec<u64>, usize)>,
}

/// One validated mutation submitted to [`Database::transact`]. Values are owned so the write
/// set can be prepared before it reaches the WAL. A batch intentionally rejects multiple
/// mutations of the same existing row: higher-level compare-and-swap/lease semantics will make
/// such ordering explicit rather than accidentally depending on request order.
#[derive(Debug, Clone)]
pub enum TransactionMutation {
    Insert {
        table: String,
        values: Vec<(String, Value)>,
    },
    Update {
        table: String,
        row_id: u64,
        values: Vec<(String, Value)>,
    },
    Delete {
        table: String,
        row_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionMutationResult {
    Inserted { row_id: u64 },
    Updated { row_id: u64 },
    Deleted { row_id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionResult {
    pub commit_lsn: u64,
    pub results: Vec<TransactionMutationResult>,
}

/// A condition evaluated under the database writer lock immediately before an atomic batch is
/// appended to the WAL. It is deliberately data-oriented rather than an arbitrary predicate so
/// callers can persist and audit the exact idempotency/CAS rule they relied upon.
#[derive(Debug, Clone)]
pub enum TransactionPrecondition {
    /// No live row in `table` may match every supplied scalar value. Runtime ingress uses this
    /// for a tenant-scoped delivery key before creating an event/state/outbox transition.
    Absent {
        table: String,
        equals: Vec<(String, Value)>,
    },
    /// The row must still contain every expected value. Runtime snapshots use an explicit
    /// `revision` column here for optimistic compare-and-swap.
    RowMatches {
        table: String,
        row_id: u64,
        equals: Vec<(String, Value)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalTransactionResult {
    pub applied: bool,
    pub transaction: Option<TransactionResult>,
}

impl HybridQuery {
    pub fn new(k: usize) -> Self {
        HybridQuery {
            k,
            ..Default::default()
        }
    }
    pub fn vector(mut self, col: &str, q: Vec<f32>) -> Self {
        self.vector = Some((col.to_string(), q));
        self
    }
    pub fn text(mut self, col: &str, q: &str) -> Self {
        self.text = Some((col.to_string(), q.to_string()));
        self
    }
    pub fn filter(mut self, col: &str, op: PredOp, v: Value) -> Self {
        self.filters.push((col.to_string(), op, v));
        self
    }
    pub fn graph(mut self, col: &str, seeds: Vec<u64>, depth: usize) -> Self {
        self.graph = Some((col.to_string(), seeds, depth));
        self
    }
}

/// A single-node database.
pub struct Database {
    dir: PathBuf,
    catalog: Catalog,
    engine: Engine,
    /// Flushed segments as resident `FileSource`s, built once at load/flush time (avoids
    /// re-reading the file and rebuilding its lookup maps on every query).
    segments: Vec<FileSource>,
    /// Segment file names, parallel to `segments`, for the manifest.
    seg_names: Vec<String>,
    next_row_id: u64,
    seg_seq: u64,
}

impl Database {
    /// Create a fresh database rooted at `dir` (which must exist). Overwrites any existing
    /// WAL; use [`Database::open`] to reopen a persisted database.
    pub fn create<P: AsRef<Path>>(dir: P) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let engine = Engine::create(dir.join(WAL_NAME), 1)?;
        Ok(Database {
            dir,
            catalog: Catalog::new(),
            engine,
            segments: Vec::new(),
            seg_names: Vec::new(),
            next_row_id: 1,
            seg_seq: 0,
        })
    }

    /// Reopen a database persisted at `dir`: load the schema and manifest, reload the
    /// flushed segments, and replay the WAL tail (records past the checkpoint) into the
    /// memtable. If there is no WAL yet, behaves like [`Database::create`].
    pub fn open<P: AsRef<Path>>(dir: P) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let wal_path = dir.join(WAL_NAME);
        if !wal_path.exists() {
            return Self::create(&dir);
        }
        let manifest = std::fs::read(dir.join(MANIFEST_NAME))
            .ok()
            .and_then(|b| Manifest::decode(&b))
            .unwrap_or_default();
        let catalog = Catalog::load(dir.join(CATALOG_NAME)).unwrap_or_else(|_| Catalog::new());
        let engine = Engine::recover_after(&wal_path, manifest.checkpoint_lsn)?;

        // Derive next_row_id defensively from every id we know about, so a missing/stale
        // manifest can never hand out an id that already exists.
        let mut max_id = manifest.next_row_id.saturating_sub(1);
        let mut segments = Vec::new();
        for name in &manifest.segments {
            let f = read_file(dir.join(name)).map_err(io::Error::other)?;
            if let Some(m) = f.translation_table.iter().copied().max() {
                max_id = max_id.max(m);
            }
            segments.push(FileSource::new(Arc::new(f)));
        }
        let seg_names = manifest.segments.clone();
        for (id, _) in engine.memtable().live_rows() {
            max_id = max_id.max(id);
        }

        Ok(Database {
            dir,
            catalog,
            engine,
            segments,
            seg_names,
            next_row_id: max_id + 1,
            seg_seq: manifest.seg_seq,
        })
    }

    /// Define a table and persist the updated schema so a reopened database can resolve its
    /// columns.
    pub fn create_table(
        &mut self,
        name: &str,
        columns: &[(&str, ColumnKind)],
    ) -> io::Result<u32> {
        let id = self
            .catalog
            .create_table(name, columns)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        self.catalog.save(self.dir.join(CATALOG_NAME))?;
        Ok(id)
    }

    /// Add nullable columns to an existing table and persist the catalog atomically. Existing
    /// rows retain their prior shape; absent values are represented as `NULL` on reads.
    pub fn add_columns(&mut self, table: &str, columns: &[(&str, ColumnKind)]) -> io::Result<()> {
        self.catalog
            .add_columns(table, columns)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        self.catalog.save(self.dir.join(CATALOG_NAME))
    }

    /// The user-defined columns of `table` as `(name, kind)`, or `None` if the table doesn't
    /// exist. Used to describe a table to clients (and to ground the NL query compiler in the
    /// real schema).
    pub fn columns(&self, table: &str) -> Option<Vec<(String, ColumnKind)>> {
        self.catalog
            .table(table)
            .map(|t| t.columns.iter().map(|c| (c.name.clone(), c.kind)).collect())
    }

    /// Atomically apply a prepared batch of inserts, updates, and deletes. Validation and row
    /// reconstruction happen before the WAL is touched; the engine then writes one transaction
    /// and exposes the whole batch at one commit point. This is the first storage building block
    /// for Aelio's event/state/ledger/outbox transaction protocol.
    pub fn transact(&mut self, mutations: Vec<TransactionMutation>) -> io::Result<TransactionResult> {
        if mutations.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "transaction requires at least one mutation"));
        }

        let mut next_row_id = self.next_row_id;
        let mut touched = BTreeSet::new();
        let mut ops = Vec::with_capacity(mutations.len());
        let mut results = Vec::with_capacity(mutations.len());

        for mutation in mutations {
            match mutation {
                TransactionMutation::Insert { table, values } => {
                    let t = self
                        .catalog
                        .table(&table)
                        .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
                    let mut row = Row::new();
                    for (name, value) in values {
                        let col = t
                            .column(&name)
                            .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
                        row.insert(col.column_id, value);
                    }
                    row.insert(SYS_TABLE_COL, Value::I64(t.table_id as i64));
                    let row_id = next_row_id;
                    next_row_id += 1;
                    ops.push(WriteOp::Insert { row_id, row });
                    results.push(TransactionMutationResult::Inserted { row_id });
                }
                TransactionMutation::Update { table, row_id, values } => {
                    if !touched.insert(row_id) {
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("row {row_id} is mutated more than once in one transaction")));
                    }
                    let t = self
                        .catalog
                        .table(&table)
                        .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
                    let Some(src_idx) = self.resolve_live(row_id) else {
                        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no live row {row_id} in {table}")));
                    };
                    if self.row_table_id(src_idx, row_id) != Some(t.table_id) {
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("row {row_id} does not belong to {table}")));
                    }
                    let Some(mut row) = self.reconstruct_row(src_idx, row_id, t.table_id) else {
                        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no reconstructable row {row_id}")));
                    };
                    for (name, value) in values {
                        let col = t
                            .column(&name)
                            .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
                        row.insert(col.column_id, value);
                    }
                    ops.push(WriteOp::Put { row_id, row });
                    results.push(TransactionMutationResult::Updated { row_id });
                }
                TransactionMutation::Delete { table, row_id } => {
                    if !touched.insert(row_id) {
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("row {row_id} is mutated more than once in one transaction")));
                    }
                    let table_id = self
                        .catalog
                        .table(&table)
                        .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?
                        .table_id;
                    let Some(src_idx) = self.resolve_live(row_id) else {
                        return Err(io::Error::new(io::ErrorKind::NotFound, format!("no live row {row_id} in {table}")));
                    };
                    if self.row_table_id(src_idx, row_id) != Some(table_id) {
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("row {row_id} does not belong to {table}")));
                    }
                    ops.push(WriteOp::Delete { row_id });
                    results.push(TransactionMutationResult::Deleted { row_id });
                }
            }
        }

        let commit_lsn = self.engine.transact(ops)?;
        self.next_row_id = next_row_id;
        Ok(TransactionResult { commit_lsn, results })
    }

    /// Apply `mutations` only if all preconditions still hold under the same exclusive writer
    /// lock. A false condition produces no WAL record and no partial write; a true condition
    /// delegates to [`transact`](Self::transact), preserving its one-commit atomicity.
    pub fn transact_conditional(
        &mut self,
        preconditions: Vec<TransactionPrecondition>,
        mutations: Vec<TransactionMutation>,
    ) -> io::Result<ConditionalTransactionResult> {
        if preconditions.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "conditional transaction requires at least one precondition"));
        }
        for condition in &preconditions {
            let holds = match condition {
                TransactionPrecondition::Absent { table, equals } => !self.has_row_with_values(table, equals)?,
                TransactionPrecondition::RowMatches { table, row_id, equals } => self.row_matches_values(table, *row_id, equals)?,
            };
            if !holds {
                return Ok(ConditionalTransactionResult { applied: false, transaction: None });
            }
        }
        let transaction = self.transact(mutations)?;
        Ok(ConditionalTransactionResult { applied: true, transaction: Some(transaction) })
    }

    fn has_row_with_values(&self, table: &str, equals: &[(String, Value)]) -> io::Result<bool> {
        if equals.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "absent precondition requires at least one equality value"));
        }
        let mut q = HybridQuery::new(1);
        for (name, value) in equals {
            q = q.filter(name, PredOp::Eq, value.clone());
        }
        Ok(!self.query(table, &q)?.is_empty())
    }

    fn row_matches_values(&self, table: &str, row_id: u64, equals: &[(String, Value)]) -> io::Result<bool> {
        if equals.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "row-match precondition requires at least one equality value"));
        }
        let Some(values) = self.get_row_values(table, row_id)? else {
            return Ok(false);
        };
        for (name, expected) in equals {
            let actual = values.iter().find(|(column, _)| column == name).map(|(_, value)| value);
            if actual != Some(expected) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Insert a row into `table` (values by column name). Returns the assigned row id.
    pub fn insert(&mut self, table: &str, values: &[(&str, Value)]) -> io::Result<u64> {
        let t = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
        let table_id = t.table_id;
        let mut row = Row::new();
        for (name, val) in values {
            let c = t
                .column(name)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
            row.insert(c.column_id, val.clone());
        }
        row.insert(SYS_TABLE_COL, Value::I64(table_id as i64));
        let row_id = self.next_row_id;
        self.next_row_id += 1;
        self.engine.insert(row_id, row)?;
        Ok(row_id)
    }

    /// Delete `row_id` from `table`. Writes a tombstone through the engine (WAL + memtable) at
    /// a fresh LSN; because that LSN is newer than any flushed copy, the global
    /// newest-version-wins resolver suppresses the row everywhere, including in already-flushed
    /// segments. Returns `Ok(false)` if the row doesn't currently exist (absent or already
    /// deleted) or belongs to another table — no tombstone is written in that case.
    pub fn delete(&mut self, table: &str, row_id: u64) -> io::Result<bool> {
        let table_id = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?
            .table_id;
        // Only delete a live row that belongs to this table.
        match self.resolve_live(row_id) {
            Some(src) if self.row_table_id(src, row_id) == Some(table_id) => {}
            _ => return Ok(false),
        }
        self.engine.delete(row_id)?;
        Ok(true)
    }

    /// Update `row_id` in `table`, overwriting the named columns and leaving the rest intact.
    /// Reconstructs the row's current (newest, live) version from whichever source holds it —
    /// memtable or a flushed segment — applies the new values, and re-inserts under the same
    /// `row_id` at a fresh LSN, so the new version supersedes the old copy via the
    /// newest-version-wins resolver. Returns `Ok(false)` if the row doesn't currently exist or
    /// belongs to another table.
    pub fn update(&mut self, table: &str, row_id: u64, values: &[(&str, Value)]) -> io::Result<bool> {
        let t = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
        let table_id = t.table_id;
        // Resolve names → column ids up front (validates the update payload against the schema).
        let mut updates: Vec<(u32, Value)> = Vec::with_capacity(values.len());
        for (name, val) in values {
            let c = t
                .column(name)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
            updates.push((c.column_id, val.clone()));
        }
        // Reconstruct the current row from its authoritative source.
        let Some(src_idx) = self.resolve_live(row_id) else {
            return Ok(false);
        };
        if self.row_table_id(src_idx, row_id) != Some(table_id) {
            return Ok(false);
        }
        let Some(mut row) = self.reconstruct_row(src_idx, row_id, table_id) else {
            return Ok(false);
        };
        for (cid, val) in updates {
            row.insert(cid, val);
        }
        self.engine.insert(row_id, row)?;
        Ok(true)
    }

    /// Flush the memtable to a new `.vss` segment and reset it, crash-safely: write+fsync the
    /// segment, record it (with the new checkpoint LSN) in the manifest atomically, then
    /// truncate the WAL. A crash between any of these steps is safe — recovery filters the
    /// WAL by the manifest's checkpoint LSN, so nothing is double-applied or lost.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.engine.memtable().is_empty() {
            return Ok(()); // nothing to persist
        }
        let name = format!("seg-{:05}.vss", self.seg_seq);
        let path = self.dir.join(&name);
        let n = self.engine.flush_to(&path)?;
        if n == 0 {
            return Ok(());
        }
        self.seg_seq += 1;
        let f = read_file(&path).map_err(io::Error::other)?;
        self.segments.push(FileSource::new(Arc::new(f)));
        self.seg_names.push(name);

        // Everything written so far is now durable in the segment.
        let new_base = self.engine.next_lsn();
        let manifest = Manifest {
            next_row_id: self.next_row_id,
            seg_seq: self.seg_seq,
            checkpoint_lsn: new_base.saturating_sub(1),
            segments: self.seg_names.clone(),
        };
        atomic_write(&self.dir.join(MANIFEST_NAME), &manifest.encode())?;
        self.engine.checkpoint_wal(new_base)?;
        self.engine.reset_memtable();
        Ok(())
    }

    /// Merge every segment **and** the memtable into a single fresh segment, dropping deleted
    /// rows (tombstones) and keeping only the newest version of each surviving row. This is how
    /// space from updates/deletes is reclaimed: tombstones, which otherwise accumulate forever
    /// (each delete persists a marker so it keeps suppressing older copies), are physically
    /// removed once no older segment can resurrect the row.
    ///
    /// Crash-safe, same discipline as [`flush`](Self::flush): write+fsync the new segment, then
    /// atomically swing the manifest to reference it alone (with an advanced checkpoint LSN),
    /// then truncate the WAL, and only then unlink the old segment files. A crash at any point
    /// leaves either the pre-compaction state (manifest still names the old segments) or the
    /// post-compaction state (manifest names the new one) — never a torn mix.
    pub fn compact(&mut self) -> io::Result<()> {
        // Materialize the newest live version of every row across all sources. `all_rows`
        // yields only live (non-deleted) ids per source; `resolve_live` then confirms the
        // *globally* newest version isn't a tombstone (a row live in an old segment but deleted
        // in a newer one is correctly dropped here).
        let merged: Vec<(u64, Row)> = {
            let mut ids: BTreeSet<u64> = BTreeSet::new();
            for s in &self.sources() {
                ids.extend(s.all_rows(SNAPSHOT_LATEST));
            }
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(src_idx) = self.resolve_live(id) {
                    if let Some(tid) = self.row_table_id(src_idx, id) {
                        if let Some(row) = self.reconstruct_row(src_idx, id, tid) {
                            out.push((id, row));
                        }
                    }
                }
            }
            out
        };

        let old_names = std::mem::take(&mut self.seg_names);
        let old_segments_empty = old_names.is_empty();
        self.segments.clear();

        // Build the new single-segment state (empty if nothing survives compaction).
        let mut new_seg_names: Vec<String> = Vec::new();
        if !merged.is_empty() {
            // A synthetic memtable of the survivors, flushed through the normal indexed path.
            // All stamped at LSN 1 — below any future WAL LSN, so later writes still supersede
            // these rows via newest-version-wins; within one segment per-row order is moot
            // because each id appears exactly once.
            let mut mem = Memtable::new();
            for (id, row) in &merged {
                mem.apply(*id, Op::Put(row.clone()), 1);
            }
            let name = format!("seg-{:05}.vss", self.seg_seq);
            let path = self.dir.join(&name);
            let n = Engine::flush_memtable_to(&mem, &path)?;
            if n > 0 {
                self.seg_seq += 1;
                let f = read_file(&path).map_err(io::Error::other)?;
                self.segments.push(FileSource::new(Arc::new(f)));
                new_seg_names.push(name);
            }
        }

        // If there was nothing to fold and nothing to drop, leave the WAL/manifest untouched.
        if merged.is_empty() && old_segments_empty {
            return Ok(());
        }

        self.seg_names = new_seg_names;
        let new_base = self.engine.next_lsn();
        let manifest = Manifest {
            next_row_id: self.next_row_id,
            seg_seq: self.seg_seq,
            checkpoint_lsn: new_base.saturating_sub(1),
            segments: self.seg_names.clone(),
        };
        atomic_write(&self.dir.join(MANIFEST_NAME), &manifest.encode())?;
        self.engine.checkpoint_wal(new_base)?;
        self.engine.reset_memtable();

        // Now that the manifest no longer references them, the old segment files are
        // unreachable — safe to unlink. A crash before this just leaves harmless orphans.
        for name in &old_names {
            if !self.seg_names.contains(name) {
                let _ = std::fs::remove_file(self.dir.join(name));
            }
        }
        Ok(())
    }

    /// Translate a name-based query into the column-id plan, adding the implicit table filter.
    fn plan(&self, table: &str, q: &HybridQuery) -> io::Result<Query> {
        let t = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
        let col = |name: &str| -> io::Result<u32> {
            t.column(name)
                .map(|c| c.column_id)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))
        };
        let mut eq = Query::new(q.k);
        if let Some((name, vec)) = &q.vector {
            eq = eq.with_vector(col(name)?, vec.clone());
        }
        if let Some((name, txt)) = &q.text {
            eq = eq.with_text(col(name)?, txt.clone());
        }
        for (name, op, val) in &q.filters {
            eq = eq.filter(col(name)?, *op, val.clone());
        }
        if let Some((name, seeds, depth)) = &q.graph {
            eq = eq.with_graph(col(name)?, seeds.clone(), *depth);
        }
        // Implicit table isolation.
        eq = eq.filter(SYS_TABLE_COL, PredOp::Eq, Value::I64(t.table_id as i64));
        Ok(eq)
    }

    /// All queryable sources: the memtable plus every resident segment.
    fn sources(&self) -> Vec<&dyn Source> {
        let mut sources: Vec<&dyn Source> = vec![self.engine.memtable()];
        for fs in &self.segments {
            sources.push(fs);
        }
        sources
    }

    /// Index into [`sources`](Self::sources) of the source holding `row_id`'s newest version
    /// visible now — but only if that newest version is live (not a tombstone). `None` if the
    /// row is absent or its newest version is deleted. This is the global newest-version-wins
    /// rule the executor uses, surfaced for mutations.
    fn resolve_live(&self, row_id: u64) -> Option<usize> {
        let sources = self.sources();
        let mut best: Option<(usize, u64, bool)> = None;
        for (i, s) in sources.iter().enumerate() {
            if let Some((stamp, deleted)) = s.version_at(row_id, SNAPSHOT_LATEST) {
                if best.is_none_or(|(_, b, _)| stamp > b) {
                    best = Some((i, stamp, deleted));
                }
            }
        }
        match best {
            Some((i, _, false)) => Some(i),
            _ => None,
        }
    }

    /// The `table_id` recorded on `row_id` in source `src_idx` (from its system column).
    fn row_table_id(&self, src_idx: usize, row_id: u64) -> Option<u32> {
        match self.sources().get(src_idx)?.scalar(row_id, SYS_TABLE_COL, SNAPSHOT_LATEST)? {
            Value::I64(t) => Some(t as u32),
            _ => None,
        }
    }

    /// Reconstruct the full row for `row_id` from source `src_idx`, by reading each of the
    /// table's columns (edges via the edge index, everything else via the column chunk) plus
    /// the system `table_id`. Used by [`update`](Self::update) and [`compact`](Self::compact)
    /// to materialize a row that may live only in a flushed segment.
    fn reconstruct_row(&self, src_idx: usize, row_id: u64, table_id: u32) -> Option<Row> {
        let sources = self.sources();
        let src = *sources.get(src_idx)?;
        let t = self.catalog.table_by_id(table_id)?;
        let mut row = Row::new();
        for c in &t.columns {
            match c.kind {
                ColumnKind::Edge => {
                    let nbrs = src.out_neighbors(c.column_id, row_id, SNAPSHOT_LATEST);
                    if !nbrs.is_empty() {
                        row.insert(c.column_id, Value::Edges(nbrs));
                    }
                }
                _ => {
                    if let Some(v) = src.scalar(row_id, c.column_id, SNAPSHOT_LATEST) {
                        row.insert(c.column_id, v);
                    }
                }
            }
        }
        row.insert(SYS_TABLE_COL, Value::I64(table_id as i64));
        Some(row)
    }

    /// Fetch a live row by id, returning named column values (for KV / state reads).
    pub fn get_row_values(&self, table: &str, row_id: u64) -> io::Result<Option<Vec<(String, Value)>>> {
        let t = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
        let table_id = t.table_id;
        let Some(src_idx) = self.resolve_live(row_id) else {
            return Ok(None);
        };
        if self.row_table_id(src_idx, row_id) != Some(table_id) {
            return Ok(None);
        }
        let Some(row) = self.reconstruct_row(src_idx, row_id, table_id) else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(t.columns.len());
        for c in &t.columns {
            if let Some(v) = row.get(&c.column_id) {
                out.push((c.name.clone(), v.clone()));
            }
        }
        Ok(Some(out))
    }

    /// Scalar-filter scan: returns up to `k` live rows with their column values.
    pub fn scan_values(&self, table: &str, q: &HybridQuery) -> io::Result<Vec<(u64, Vec<(String, Value)>)>> {
        let hits = self.query(table, q)?;
        let mut out = Vec::with_capacity(hits.len());
        for (row_id, _) in hits {
            if let Some(vals) = self.get_row_values(table, row_id)? {
                out.push((row_id, vals));
            }
        }
        Ok(out)
    }

    /// Run a hybrid query over `table`, fusing recent (memtable) and persisted (segments)
    /// data across all modalities. Returns top-k `(row_id, score)`.
    pub fn query(&self, table: &str, q: &HybridQuery) -> io::Result<Vec<(u64, f32)>> {
        let plan = self.plan(table, q)?;
        Ok(execute(&self.sources(), &plan, SNAPSHOT_LATEST))
    }

    /// A human-readable plan description, including the cost-model-chosen vector strategy.
    pub fn explain(&self, table: &str, q: &HybridQuery) -> io::Result<String> {
        let plan = self.plan(table, q)?;
        Ok(explain_plan(&self.sources(), &plan, SNAPSHOT_LATEST))
    }
}
