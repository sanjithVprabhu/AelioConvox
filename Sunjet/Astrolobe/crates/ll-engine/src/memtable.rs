//! The memtable: an in-memory, MVCC write absorber keyed by row id.
//!
//! v0 is a `BTreeMap` row store (ordered by row id). Each row keeps a small version chain
//! (`xmin`/`xmax` by LSN); a snapshot read at LSN `T` sees the version with
//! `xmin <= T < xmax`. A delete pushes a tombstone version. (A lock-free skiplist and the
//! per-modality in-memory index analogs from the spec are later additions.)

use std::collections::BTreeMap;

use crate::value::Value;

/// A row: column id → value.
pub type Row = BTreeMap<u32, Value>;

/// "live forever" sentinel for `xmax`.
pub const INF: u64 = u64::MAX;

/// A write operation against a row id.
pub enum Op {
    Put(Row),
    Delete,
}

#[derive(Debug, Clone)]
struct Version {
    xmin: u64,
    xmax: u64,
    /// `None` = tombstone (deleted).
    row: Option<Row>,
}

/// In-memory MVCC row store.
#[derive(Debug, Default)]
pub struct Memtable {
    rows: BTreeMap<u64, Vec<Version>>,
    approx_bytes: usize,
    min_lsn: u64,
    max_lsn: u64,
}

impl Memtable {
    pub fn new() -> Self {
        Memtable {
            rows: BTreeMap::new(),
            approx_bytes: 0,
            min_lsn: u64::MAX,
            max_lsn: 0,
        }
    }

    /// Apply a committed write at `lsn`. Supersedes the row's current open version.
    pub fn apply(&mut self, row_id: u64, op: Op, lsn: u64) {
        let versions = self.rows.entry(row_id).or_default();
        if let Some(last) = versions.last_mut() {
            if last.xmax == INF {
                last.xmax = lsn;
            }
        }
        match op {
            Op::Put(row) => {
                self.approx_bytes += 16 + row.values().map(Value::approx_bytes).sum::<usize>();
                versions.push(Version {
                    xmin: lsn,
                    xmax: INF,
                    row: Some(row),
                });
            }
            Op::Delete => versions.push(Version {
                xmin: lsn,
                xmax: INF,
                row: None,
            }),
        }
        self.min_lsn = self.min_lsn.min(lsn);
        self.max_lsn = self.max_lsn.max(lsn);
    }

    fn visible_at(versions: &[Version], t: u64) -> Option<&Row> {
        versions
            .iter()
            .rev()
            .find(|v| v.xmin <= t && t < v.xmax)
            .and_then(|v| v.row.as_ref())
    }

    /// Snapshot LSN that sees all committed writes so far (read-latest).
    pub fn snapshot_lsn(&self) -> u64 {
        self.max_lsn.saturating_add(1)
    }

    /// Visible row version at `snapshot`, or `None` (absent or tombstoned).
    pub fn get(&self, row_id: u64, snapshot: u64) -> Option<&Row> {
        self.rows.get(&row_id).and_then(|vs| Self::visible_at(vs, snapshot))
    }

    /// The newest version of `row_id` visible at `snapshot`, as `(stamp, deleted)` — `stamp`
    /// is its creation LSN (globally monotonic; higher = newer) and `deleted` marks a
    /// tombstone. `None` if no version is visible. The executor uses this to resolve
    /// newest-version-wins across the memtable and flushed segments.
    pub fn version_at(&self, row_id: u64, snapshot: u64) -> Option<(u64, bool)> {
        self.rows.get(&row_id).and_then(|vs| {
            vs.iter()
                .rev()
                .find(|v| v.xmin <= snapshot && snapshot < v.xmax)
                .map(|v| (v.xmin, v.row.is_none()))
        })
    }

    /// All rows visible at `snapshot`, ascending by row id.
    pub fn scan(&self, snapshot: u64) -> Vec<(u64, &Row)> {
        self.rows
            .iter()
            .filter_map(|(id, vs)| Self::visible_at(vs, snapshot).map(|r| (*id, r)))
            .collect()
    }

    /// Currently-live rows (snapshot = latest).
    pub fn live_rows(&self) -> Vec<(u64, &Row)> {
        self.scan(self.snapshot_lsn())
    }

    /// Currently-live rows with each one's `xmin` (creation LSN), for flush.
    pub fn live_rows_versioned(&self) -> Vec<(u64, u64, &Row)> {
        let t = self.snapshot_lsn();
        self.rows
            .iter()
            .filter_map(|(id, vs)| {
                vs.iter()
                    .rev()
                    .find(|v| v.xmin <= t && t < v.xmax)
                    .and_then(|v| v.row.as_ref().map(|r| (*id, v.xmin, r)))
            })
            .collect()
    }

    /// Every row with a version visible at the latest snapshot, **including tombstones**, as
    /// `(row_id, xmin, Option<&Row>)` (`None` = tombstone). Flush uses this so a delete is
    /// persisted into the new segment and continues to suppress the row's stale copy in any
    /// older segment — otherwise dropping the tombstone at flush would resurrect it.
    pub fn visible_entries_versioned(&self) -> Vec<(u64, u64, Option<&Row>)> {
        let t = self.snapshot_lsn();
        self.rows
            .iter()
            .filter_map(|(id, vs)| {
                vs.iter()
                    .rev()
                    .find(|v| v.xmin <= t && t < v.xmax)
                    .map(|v| (*id, v.xmin, v.row.as_ref()))
            })
            .collect()
    }

    pub fn live_count(&self) -> usize {
        self.live_rows().len()
    }
    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }
    pub fn min_lsn(&self) -> u64 {
        if self.min_lsn == u64::MAX {
            0
        } else {
            self.min_lsn
        }
    }
    pub fn max_lsn(&self) -> u64 {
        self.max_lsn
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}
