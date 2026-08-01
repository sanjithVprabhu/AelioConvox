//! `aelio-db-query` — LL's query layer ("one query plan").
//!
//! The [`Source`] abstraction (per-modality search returning `(row_id, score)` at a
//! snapshot) is implemented for the in-memory [`aelio_db_engine::Memtable`] and for flushed
//! `.vss` files ([`FileSource`]). [`execute`] runs a hybrid [`Query`] — vector + text +
//! graph + scalar filters as first-class operators — across **all** sources, merging by
//! global `row_id` and fusing per-modality rankings with Reciprocal Rank Fusion. The
//! [`Database`] facade ties the catalog, engine, and segments into a `query()` over named
//! tables.
//!
//! This is the integration thesis made real: relational, vector, fuaelio-db-text, and graph data
//! in one substrate, queried through one plan over recent and persisted data, with MVCC.
//!
//! Remaining database-wide work beyond Prism: persist live per-column statistics so the
//! bounded-memory selectivity sample can become sub-linear, explicit transactions, and a
//! DataFusion swap for SQL + the relational tail.

mod database;
mod exec;
mod file_source;
mod prism_exec;
mod source;
mod util;

pub use database::{Database, DatabaseQueryError, HybridQuery, InsertIfAbsent, UpdateIfVersion};
pub use exec::{
    execute, execute_checked, explain_plan, GraphBudget, GraphBudgetExceeded, GraphBudgetKind,
    GraphConstraint, PredOp, Predicate, Query, QueryError,
};
pub use file_source::FileSource;
pub use prism_exec::{
    execute_prism, lower_prism, project_row, HashEmbedder, PrismEmbedder, PrismExecError, PrismHit,
};
pub use source::Source;

// Re-exports for ergonomic query construction.
pub use aelio_db_catalog::ColumnKind;
pub use aelio_db_engine::Value;
pub use aelio_db_storage::{
    build_store, from_env as storage_from_env, LocalSegmentStore, S3SegmentStore, SegmentBackend,
    SegmentStore, StorageConfig,
};
