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

use aelio_db_catalog::{Catalog, ColumnKind};
use aelio_db_engine::{Engine, Memtable, Op, Row, Value};
use aelio_db_format::read_file;
use aelio_db_storage::{LocalSegmentStore, SegmentStore};

use crate::exec::{
    execute, execute_checked, explain_plan, GraphBudget, PredOp, Predicate, Query, QueryError,
};
use crate::file_source::FileSource;
use crate::source::Source;

/// System column holding a row's `table_id` (distinct from `aelio_db_engine::SYS_XMIN_COL`).
const SYS_TABLE_COL: u32 = u32::MAX - 1;
/// Snapshot that sees all committed data (one below the `xmax = ∞` sentinel).
const SNAPSHOT_LATEST: u64 = u64::MAX - 1;

const WAL_NAME: &str = "wal.log";
const CATALOG_NAME: &str = "catalog.bin";
const MANIFEST_NAME: &str = "manifest.bin";
const MANIFEST_MAGIC: u32 = u32::from_le_bytes(*b"LMAN");
const MAX_CELL_BYTES: usize = 1 << 20;
const MAX_EDGE_VALUES: usize = 10_000;

fn validate_cell(table: &str, name: &str, kind: ColumnKind, value: &Value) -> io::Result<()> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    let valid = matches!(
        (kind, value),
        (ColumnKind::Bool, Value::Bool(_))
            | (ColumnKind::I64 | ColumnKind::Timestamp, Value::I64(_))
            | (ColumnKind::F64, Value::F64(_))
            | (ColumnKind::Utf8 | ColumnKind::Text, Value::Utf8(_))
            | (ColumnKind::Vector(_), Value::Vector(_))
            | (ColumnKind::Edge, Value::Edges(_))
    );
    if !valid {
        return Err(io::Error::other(format!(
            "value type does not match schema for {table}.{name} ({kind:?})"
        )));
    }
    match (kind, value) {
        (ColumnKind::F64, Value::F64(number)) if !number.is_finite() => Err(io::Error::other(
            format!("non-finite float for {table}.{name}"),
        )),
        (ColumnKind::Utf8 | ColumnKind::Text, Value::Utf8(text)) if text.len() > MAX_CELL_BYTES => {
            Err(io::Error::other(format!(
                "value for {table}.{name} exceeds {MAX_CELL_BYTES} UTF-8 bytes"
            )))
        }
        (ColumnKind::Vector(dim), Value::Vector(vector)) if vector.len() != dim as usize => {
            Err(io::Error::other(format!(
                "vector for {table}.{name} has dimension {}, expected {dim}",
                vector.len()
            )))
        }
        (ColumnKind::Vector(_), Value::Vector(vector))
            if vector.iter().any(|number| !number.is_finite()) =>
        {
            Err(io::Error::other(format!(
                "vector for {table}.{name} contains a non-finite value"
            )))
        }
        (ColumnKind::Edge, Value::Edges(edges)) if edges.len() > MAX_EDGE_VALUES => {
            Err(io::Error::other(format!(
                "edge list for {table}.{name} exceeds {MAX_EDGE_VALUES} values"
            )))
        }
        _ => Ok(()),
    }
}

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
#[derive(Debug, Clone)]
pub struct HybridQuery {
    pub k: usize,
    pub vector: Option<(String, Vec<f32>)>,
    pub text: Option<(String, String)>,
    pub filters: Vec<(String, PredOp, Value)>,
    pub graph: Option<(String, Vec<u64>, usize)>,
    pub graph_budget: GraphBudget,
    pub graph_scope_filters: bool,
    /// Reciprocal Rank Fusion constant (default 60). Set from Prism `match fusion`.
    pub rrf_k: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertIfAbsent {
    Inserted { row_id: u64, version: u64 },
    Existing { row_id: u64, version: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateIfVersion {
    Updated { version: u64 },
    Conflict { current_version: u64 },
    NotFound,
}

pub type NamedRow = Vec<(String, Value)>;

impl HybridQuery {
    pub fn new(k: usize) -> Self {
        HybridQuery {
            k,
            vector: None,
            text: None,
            filters: Vec::new(),
            graph: None,
            graph_budget: GraphBudget::default(),
            graph_scope_filters: false,
            rrf_k: 60.0,
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
    pub fn graph_budget(mut self, budget: GraphBudget) -> Self {
        self.graph_budget = budget;
        self
    }
    /// Apply scalar predicates to every traversed node, not only final candidates.
    pub fn graph_scope_filters(mut self) -> Self {
        self.graph_scope_filters = true;
        self
    }
    pub fn rrf_k(mut self, k: f32) -> Self {
        self.rrf_k = k;
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
    /// Where immutable `.vss` segments are published / fetched / pruned.
    /// Local-only by default; cloud backends write-through to object storage.
    store: Arc<dyn SegmentStore>,
}

impl Database {
    /// Create a fresh database rooted at `dir` (which must exist). Overwrites any existing
    /// WAL; use [`Database::open`] to reopen a persisted database.
    pub fn create<P: AsRef<Path>>(dir: P) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let store: Arc<dyn SegmentStore> = Arc::new(LocalSegmentStore::new(&dir));
        Self::create_with_store(dir, store)
    }

    /// Like [`create`](Self::create) but with an explicit segment store (e.g. S3-backed).
    pub fn create_with_store(dir: PathBuf, store: Arc<dyn SegmentStore>) -> io::Result<Self> {
        let engine = Engine::create(dir.join(WAL_NAME), 1)?;
        Ok(Database {
            dir,
            catalog: Catalog::new(),
            engine,
            segments: Vec::new(),
            seg_names: Vec::new(),
            next_row_id: 1,
            seg_seq: 0,
            store,
        })
    }

    /// Reopen a database persisted at `dir`: load the schema and manifest, reload the
    /// flushed segments, and replay the WAL tail (records past the checkpoint) into the
    /// memtable. If there is no WAL yet, behaves like [`Database::create`].
    pub fn open<P: AsRef<Path>>(dir: P) -> io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let store: Arc<dyn SegmentStore> = Arc::new(LocalSegmentStore::new(&dir));
        Self::open_with_store(dir, store)
    }

    /// Like [`open`](Self::open) but with an explicit segment store. Missing local
    /// segments are fetched via [`SegmentStore::ensure_local`] (cloud download).
    pub fn open_with_store(dir: PathBuf, store: Arc<dyn SegmentStore>) -> io::Result<Self> {
        let wal_path = dir.join(WAL_NAME);
        if !wal_path.exists() {
            return Self::create_with_store(dir, store);
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
            let path = dir.join(name);
            store.ensure_local(name, &path)?;
            let f = read_file(&path).map_err(io::Error::other)?;
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
            store,
        })
    }

    /// The active segment backend label (`local`, `s3`, `cached`, …).
    pub fn segment_backend(&self) -> &'static str {
        self.store.backend_name()
    }

    /// Define a table and persist the updated schema so a reopened database can resolve its
    /// columns.
    pub fn create_table(&mut self, name: &str, columns: &[(&str, ColumnKind)]) -> io::Result<u32> {
        validate_schema_name("table", name)?;
        if columns.is_empty() || columns.len() > 1_024 {
            return Err(io::Error::other(
                "table must declare between 1 and 1024 columns",
            ));
        }
        let mut seen = BTreeSet::new();
        for (column, kind) in columns {
            validate_schema_name("column", column)?;
            if !seen.insert(*column) {
                return Err(io::Error::other(format!(
                    "duplicate column `{column}` in table `{name}`"
                )));
            }
            if matches!(kind, ColumnKind::Vector(0)) {
                return Err(io::Error::other(format!(
                    "vector column `{column}` must have a positive dimension"
                )));
            }
        }
        let id = self
            .catalog
            .create_table(name, columns)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        self.catalog.save(self.dir.join(CATALOG_NAME))?;
        Ok(id)
    }

    /// The user-defined columns of `table` as `(name, kind)`, or `None` if the table doesn't
    /// exist. Used to describe a table to clients (and to ground the NL query compiler in the
    /// real schema).
    pub fn columns(&self, table: &str) -> Option<Vec<(String, ColumnKind)>> {
        self.catalog
            .table(table)
            .map(|t| t.columns.iter().map(|c| (c.name.clone(), c.kind)).collect())
    }

    /// Insert a row into `table` (values by column name). Returns the assigned row id.
    pub fn insert(&mut self, table: &str, values: &[(&str, Value)]) -> io::Result<u64> {
        let t = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
        let table_id = t.table_id;
        let mut row = Row::new();
        let mut seen = BTreeSet::new();
        for (name, val) in values {
            if !seen.insert(*name) {
                return Err(io::Error::other(format!(
                    "duplicate column in insert: {table}.{name}"
                )));
            }
            let c = t
                .column(name)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
            validate_cell(table, name, c.kind, val)?;
            row.insert(c.column_id, val.clone());
        }
        row.insert(SYS_TABLE_COL, Value::I64(table_id as i64));
        let row_id = self.next_row_id;
        self.next_row_id += 1;
        self.engine.insert(row_id, row)?;
        Ok(row_id)
    }

    /// Atomically insert a row only when no live row matches all equality conditions.
    pub fn insert_if_absent(
        &mut self,
        table: &str,
        conditions: &[(&str, Value)],
        values: &[(&str, Value)],
    ) -> io::Result<InsertIfAbsent> {
        if conditions.is_empty() {
            return Err(io::Error::other(
                "insert_if_absent requires at least one condition",
            ));
        }
        let query = conditions
            .iter()
            .fold(HybridQuery::new(1), |query, (name, value)| {
                query.filter(name, PredOp::Eq, value.clone())
            });
        if let Some((row_id, _)) = self.query(table, &query)?.into_iter().next() {
            let version = self
                .row_version(table, row_id)?
                .ok_or_else(|| io::Error::other("matched row version is unavailable"))?;
            return Ok(InsertIfAbsent::Existing { row_id, version });
        }
        let row_id = self.insert(table, values)?;
        let version = self
            .row_version(table, row_id)?
            .ok_or_else(|| io::Error::other("inserted row version is unavailable"))?;
        Ok(InsertIfAbsent::Inserted { row_id, version })
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
    pub fn update(
        &mut self,
        table: &str,
        row_id: u64,
        values: &[(&str, Value)],
    ) -> io::Result<bool> {
        let t = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
        let table_id = t.table_id;
        // Resolve names → column ids up front (validates the update payload against the schema).
        let mut updates: Vec<(u32, Value)> = Vec::with_capacity(values.len());
        let mut seen = BTreeSet::new();
        for (name, val) in values {
            if !seen.insert(*name) {
                return Err(io::Error::other(format!(
                    "duplicate column in update: {table}.{name}"
                )));
            }
            let c = t
                .column(name)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
            validate_cell(table, name, c.kind, val)?;
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

    /// Compare-and-set a live row using its globally monotonic MVCC version.
    pub fn update_if_version(
        &mut self,
        table: &str,
        row_id: u64,
        expected_version: u64,
        values: &[(&str, Value)],
    ) -> io::Result<UpdateIfVersion> {
        let Some(current_version) = self.row_version(table, row_id)? else {
            return Ok(UpdateIfVersion::NotFound);
        };
        if current_version != expected_version {
            return Ok(UpdateIfVersion::Conflict { current_version });
        }
        if !self.update(table, row_id, values)? {
            return Ok(UpdateIfVersion::NotFound);
        }
        let version = self
            .row_version(table, row_id)?
            .ok_or_else(|| io::Error::other("updated row version is unavailable"))?;
        Ok(UpdateIfVersion::Updated { version })
    }

    /// Flush the memtable to a new `.vss` segment and reset it, crash-safely: write+fsync the
    /// segment, publish it to the configured [`SegmentStore`] (local and/or cloud), record it
    /// (with the new checkpoint LSN) in the manifest atomically, then truncate the WAL. A crash
    /// between any of these steps is safe — recovery filters the WAL by the manifest's
    /// checkpoint LSN, so nothing is double-applied or lost.
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
        // Publish BEFORE swinging the manifest so a crash never leaves the manifest pointing
        // at a segment that isn't durable in the configured store.
        self.store.publish(&name, &path)?;
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
                self.store.publish(&name, &path)?;
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
        // unreachable — safe to prune from local disk AND the cloud store. A crash before
        // this just leaves harmless orphans (local and/or remote).
        for name in &old_names {
            if !self.seg_names.contains(name) {
                let _ = self.store.remove(name, &self.dir.join(name));
            }
        }
        Ok(())
    }

    /// Translate a name-based query into the column-id plan, adding the implicit table filter.
    fn plan(&self, table: &str, q: &HybridQuery) -> io::Result<Query> {
        if q.k == 0 {
            return Err(io::Error::other("query limit must be positive"));
        }
        if !q.rrf_k.is_finite() || q.rrf_k < 0.0 || q.rrf_k > 1_000_000.0 {
            return Err(io::Error::other("RRF k must be finite and in 0..=1000000"));
        }
        let t = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?;
        let mut eq = Query::new(q.k);
        eq.rrf_k = q.rrf_k;
        if let Some((name, vec)) = &q.vector {
            let definition = t
                .column(name)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
            let ColumnKind::Vector(dim) = definition.kind else {
                return Err(io::Error::other(format!(
                    "vector search requires a vector column: {table}.{name}"
                )));
            };
            if vec.len() != dim as usize || vec.iter().any(|value| !value.is_finite()) {
                return Err(io::Error::other(format!(
                    "query vector for {table}.{name} must contain {dim} finite values"
                )));
            }
            eq = eq.with_vector(definition.column_id, vec.clone());
        }
        if let Some((name, txt)) = &q.text {
            let definition = t
                .column(name)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
            if definition.kind != ColumnKind::Text {
                return Err(io::Error::other(format!(
                    "text search requires a text column: {table}.{name}"
                )));
            }
            if txt.trim().is_empty() || txt.len() > 64 * 1024 {
                return Err(io::Error::other(
                    "text query must be non-empty and at most 65536 UTF-8 bytes",
                ));
            }
            eq = eq.with_text(definition.column_id, txt.clone());
        }
        if let Some((name, seeds, depth)) = &q.graph {
            let definition = t
                .column(name)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
            if definition.kind != ColumnKind::Edge {
                return Err(io::Error::other(format!(
                    "graph traversal requires an edge column: {table}.{name}"
                )));
            }
            eq = eq
                .with_graph(definition.column_id, seeds.clone(), *depth)
                .with_graph_budget(q.graph_budget)
                .with_graph_scope(Predicate {
                    col: SYS_TABLE_COL,
                    op: PredOp::Eq,
                    value: Value::I64(t.table_id as i64),
                });
        }
        if q.filters.len() > 64 {
            return Err(io::Error::other("query may contain at most 64 filters"));
        }
        for (name, op, val) in &q.filters {
            let definition = t
                .column(name)
                .ok_or_else(|| io::Error::other(format!("no such column: {table}.{name}")))?;
            if matches!(definition.kind, ColumnKind::Vector(_) | ColumnKind::Edge) {
                return Err(io::Error::other(format!(
                    "scalar filter cannot target {table}.{name} ({:?})",
                    definition.kind
                )));
            }
            validate_cell(table, name, definition.kind, val)?;
            if matches!(val, Value::Null) && !matches!(op, PredOp::Eq | PredOp::Ne) {
                return Err(io::Error::other(format!(
                    "ordered comparison on {table}.{name} may not use null"
                )));
            }
            let predicate = Predicate {
                col: definition.column_id,
                op: *op,
                value: val.clone(),
            };
            eq = eq.filter(predicate.col, predicate.op, predicate.value.clone());
            if q.graph_scope_filters {
                eq = eq.with_graph_scope(predicate);
            }
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
        match self
            .sources()
            .get(src_idx)?
            .scalar(row_id, SYS_TABLE_COL, SNAPSHOT_LATEST)?
        {
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
    pub fn get_row_values(
        &self,
        table: &str,
        row_id: u64,
    ) -> io::Result<Option<Vec<(String, Value)>>> {
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

    /// Return the current live MVCC version for a row belonging to `table`.
    pub fn row_version(&self, table: &str, row_id: u64) -> io::Result<Option<u64>> {
        let table_id = self
            .catalog
            .table(table)
            .ok_or_else(|| io::Error::other(format!("no such table: {table}")))?
            .table_id;
        let Some(source_index) = self.resolve_live(row_id) else {
            return Ok(None);
        };
        if self.row_table_id(source_index, row_id) != Some(table_id) {
            return Ok(None);
        }
        Ok(self.sources()[source_index]
            .version_at(row_id, SNAPSHOT_LATEST)
            .map(|(version, _)| version))
    }

    /// Scalar-filter scan: returns up to `k` live rows with their column values.
    pub fn scan_values(&self, table: &str, q: &HybridQuery) -> io::Result<Vec<(u64, NamedRow)>> {
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

    /// Run a query while preserving typed graph-budget failures.
    pub fn query_checked(
        &self,
        table: &str,
        q: &HybridQuery,
    ) -> Result<Vec<(u64, f32)>, DatabaseQueryError> {
        let plan = self.plan(table, q).map_err(DatabaseQueryError::Planning)?;
        execute_checked(&self.sources(), &plan, SNAPSHOT_LATEST)
            .map_err(DatabaseQueryError::Execution)
    }

    /// A human-readable plan description, including the cost-model-chosen vector strategy.
    pub fn explain(&self, table: &str, q: &HybridQuery) -> io::Result<String> {
        let plan = self.plan(table, q)?;
        Ok(explain_plan(&self.sources(), &plan, SNAPSHOT_LATEST))
    }
}

fn validate_schema_name(kind: &str, name: &str) -> io::Result<()> {
    if name.trim().is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
        return Err(io::Error::other(format!(
            "{kind} name must be non-empty, at most 255 UTF-8 bytes, and contain no control characters"
        )));
    }
    Ok(())
}

#[derive(Debug)]
pub enum DatabaseQueryError {
    Planning(io::Error),
    Execution(QueryError),
}

impl std::fmt::Display for DatabaseQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Planning(error) => write!(f, "query planning failed: {error}"),
            Self::Execution(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DatabaseQueryError {}
