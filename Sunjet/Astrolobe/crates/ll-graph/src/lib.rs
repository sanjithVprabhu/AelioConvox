//! `ll-graph` — LL's graph (edge) index.
//!
//! Forward + reverse CSR adjacency with a bloom filter over targets (for cross-file
//! reverse-traversal pruning) and depth-limited forward traversal. Edges store global
//! target row ids (D-001). An on-disk `Edge` section embeds in a `.vss` file via
//! `ll-format`. Per-edge MVCC and edge properties are later additions.

mod bloom;
mod index;
mod page;

pub use bloom::Bloom;
pub use index::EdgeIndex;
pub use page::{serialize_edge_index, EdgeError, EdgeView};
