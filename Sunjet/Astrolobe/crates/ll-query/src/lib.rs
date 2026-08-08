//! `ll-query` — LL's query layer ("one query plan").
//!
//! The [`Source`] abstraction (per-modality search returning `(row_id, score)` at a
//! snapshot) is implemented for the in-memory [`ll_engine::Memtable`] and for flushed
//! `.vss` files ([`FileSource`]). [`execute`] runs a hybrid [`Query`] — vector + text +
//! graph + scalar filters as first-class operators — across **all** sources, merging by
//! global `row_id` and fusing per-modality rankings with Reciprocal Rank Fusion. The
//! [`Database`] facade ties the catalog, engine, and segments into a `query()` over named
//! tables.
//!
//! This is the integration thesis made real: relational, vector, full-text, and graph data
//! in one substrate, queried through one plan over recent and persisted data, with MVCC.
//!
//! Remaining: cost-model-driven strategy selection at execution time (the `ll-cost`
//! selectivity estimator is built and validated; wiring it to live per-column stats is the
//! next step), explicit transactions, and a DataFusion swap for SQL + the relational tail.

mod database;
mod exec;
mod file_source;
mod source;
mod util;

pub use database::{
    ConditionalTransactionResult, Database, HybridQuery, TransactionMutation,
    TransactionMutationResult, TransactionPrecondition, TransactionResult,
};
pub use exec::{execute, explain_plan, GraphConstraint, PredOp, Predicate, Query};
pub use file_source::FileSource;
pub use source::Source;

// Re-exports for ergonomic query construction.
pub use ll_catalog::ColumnKind;
pub use ll_engine::Value;
