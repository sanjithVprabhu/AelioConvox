//! `aelio-db-engine` — the engine spine.
//!
//! The [`Memtable`] is an MVCC in-memory write absorber; the [`Engine`] ties the WAL
//! (`aelio-db-wal`) to it and flushes to a VSS file (`aelio-db-format`). This closes the write loop:
//! a write is durable in the WAL, queryable in the memtable, recoverable after a crash,
//! and persistable on flush.
//!
//! v0: single-threaded `BTreeMap` memtable, auto-commit writes, full-file flush. The
//! lock-free skiplist, per-modality in-memory index analogs, frozen memtables/background
//! flush, explicit transactions, and catalog integration are later milestones.

mod codec;
mod engine;
mod memtable;
mod value;

pub use engine::{Engine, SYS_XMAX_COL, SYS_XMIN_COL};
pub use memtable::{Memtable, Op, Row, INF};
pub use value::Value;
