//! `ll-format` — the VSS on-disk file format (Versioned Storage Substrate, `.vss`).
//!
//! This crate owns the bytes of an immutable VSS data file: the preamble, column-chunk
//! and embedded-index regions, the section directory, the footer, and the three-layer
//! CRC32C checksums. It is the foundation every other layer (WAL, memtable, query
//! engine) reads and writes through.
//!
//! See `TechSpec/LL_FileFormat_ByteLayout.md` for the byte-level specification and
//! `TechSpec/LL_Decisions_Delta.md` for decisions that supersede the architecture spec.
//!
//! ## Current scope (v0 spine)
//! Implemented end-to-end with checksum validation: preamble, column chunks (scalar and
//! vector, with zone maps), the [`SectionType::TranslationTable`] section, the embedded
//! HNSW / text / edge index sections (carried as opaque [`RawSection`] regions written and
//! read back verbatim), the footer, and the trailer — via [`write_file`]/[`read_file`] and
//! [`write_file_with`]. The `OptimizerStats` section is reserved but not yet populated.

mod codec;

pub mod checksum;
pub mod error;

mod column;
mod file;
mod footer;
mod preamble;
mod types;

pub use checksum::crc32c;
pub use column::{Column, DEFAULT_ROWS_PER_PAGE};
pub use error::{FormatError, Result};
pub use file::{
    read_file, write_file, write_file_with, FileMeta, LlFile, RawSection, WriteOptions,
    TRAILER_LEN,
};
pub use footer::{Footer, MvccSummary, SectionEncoding, SectionEntry, SectionType, ZoneMap};
pub use preamble::{Preamble, FORMAT_VERSION, MAGIC, PREAMBLE_LEN};
pub use types::{ColumnValues, LogicalType};
