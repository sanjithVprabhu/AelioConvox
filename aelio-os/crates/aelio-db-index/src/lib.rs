//! `aelio-db-index` — index algorithms for LL.
//!
//! Currently: in-memory HNSW (build + search). Upcoming: int8 quantization with
//! full-precision rerank, the on-disk HNSW page layout (byte-layout §3.1), and BFS
//! reordering. The on-disk format framing itself lives in `aelio-db-format`; this crate owns
//! the index *algorithms* and (later) the parsing of index section bytes.

mod filtered;
mod hnsw;
mod page;
mod quant;
mod rng;
mod simd;
mod stats;

pub use filtered::{integrated, postfilter, prefilter, FilteredResult};
pub use hnsw::{distance, Hnsw, HnswParams, Metric};
pub use page::{serialize_hnsw, HnswView, IndexError, Neighbor};
pub use quant::ScalarQuantizer;
pub use rng::SplitMix64;
pub use simd::l2_squared;
pub use stats::{decode_recall_curve, encode_recall_curve, RECALL_EF_LADDER};
