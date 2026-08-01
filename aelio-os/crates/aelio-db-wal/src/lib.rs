//! `aelio-db-wal` — LL's write-ahead log.
//!
//! One unified, LSN-ordered, CRC'd log for every write (the spec's "one WAL"). Logical
//! records (`INSERT/UPDATE/DELETE_ROW`, `COMMIT`, `ABORT`, `CHECKPOINT`) are appended and
//! made durable with fsync at the commit point; replay returns every valid record up to
//! the crash point. Group commit, file rotation, and checkpoint truncation come later.

mod record;
mod wal;

pub use record::{Record, RecordType};
pub use wal::{replay, Replay, Wal, WAL_MAGIC, WAL_VERSION};
