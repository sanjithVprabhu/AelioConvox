//! `ll-catalog` — LL's catalog: the connective tissue.
//!
//! Tracks tables, their typed columns (scalar / vector / text / edge — telling the planner
//! how each is indexed and queried), and the `.vss` files that hold each table's persisted
//! rows. In-memory and fully cached, with atomic file persistence. This is what the query
//! planner consults to know what tables/columns/indexes exist and which files to fan out
//! to. (redb backing, schema versioning, and the model registry are later additions.)

mod catalog;
mod schema;

pub use catalog::{Catalog, CatalogError};
pub use schema::{ColumnDef, ColumnKind, FileRef, TableDef};
