//! The file spine: write/read a complete LL file consisting of the preamble, the
//! column-chunk sections, the TranslationTable section, the footer, and the 16-byte
//! trailer.
//!
//! Embedded-index sections (HNSW/text/edge) are upcoming milestones; they slot in as
//! additional sections without changing this code's shape.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::checksum::crc32c;
use crate::column::{decode_column_chunk, encode_column_chunk, Column, DEFAULT_ROWS_PER_PAGE};
use crate::error::{FormatError, Result};
use crate::footer::{Footer, MvccSummary, SectionEncoding, SectionEntry, SectionType};
use crate::preamble::{Preamble, FORMAT_VERSION, MAGIC, PREAMBLE_LEN};
use crate::types::ColumnValues;

/// Size of the fixed trailer at the end of the file.
pub const TRAILER_LEN: usize = 16;

/// Tunables for writing a file.
#[derive(Debug, Clone)]
pub struct WriteOptions {
    /// Maximum rows per column-chunk page.
    pub rows_per_page: u32,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            rows_per_page: DEFAULT_ROWS_PER_PAGE,
        }
    }
}

/// Everything the caller supplies to write a file (besides the columns and translation).
#[derive(Debug, Clone)]
pub struct FileMeta {
    pub file_uuid: [u8; 16],
    pub min_lsn: u64,
    pub max_lsn: u64,
    pub row_count: u64,
    pub schema_fingerprint: u64,
    pub creation_unix_nanos: u64,
    pub schema_blob: Vec<u8>,
    pub mvcc: MvccSummary,
    pub writer_version: String,
}

/// An opaque (non-column) section's bytes — e.g. an HNSW / text / edge index produced by
/// another crate. `ll-format` only frames and CRC-verifies these; it does not interpret
/// them.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSection {
    pub kind: SectionType,
    pub column_id: u32,
    pub encoding: SectionEncoding,
    pub bytes: Vec<u8>,
}

/// A fully-read VSS file.
#[derive(Debug, Clone, PartialEq)]
pub struct LlFile {
    pub preamble: Preamble,
    pub footer: Footer,
    /// Column chunks in section order.
    pub columns: Vec<Column>,
    /// Opaque index/aux sections (HNSW, text, edge, optimizer stats), CRC-verified.
    pub raw_sections: Vec<RawSection>,
    /// `translation_table[local_offset] == global_row_id`.
    pub translation_table: Vec<u64>,
}

impl LlFile {
    /// Bytes of the first section matching `(kind, column_id)`, if present.
    pub fn section(&self, kind: SectionType, column_id: u32) -> Option<&[u8]> {
        self.raw_sections
            .iter()
            .find(|s| s.kind == kind && s.column_id == column_id)
            .map(|s| s.bytes.as_slice())
    }
}

/// Write a complete file at `path` with default [`WriteOptions`] and no extra sections.
pub fn write_file<P: AsRef<Path>>(
    path: P,
    meta: &FileMeta,
    columns: &[Column],
    translation_table: &[u64],
) -> Result<()> {
    write_file_with(
        path,
        meta,
        columns,
        translation_table,
        &[],
        &WriteOptions::default(),
    )
}

/// Write a complete file at `path` (atomically via temp-then-rename).
///
/// `extra` carries opaque pre-serialized sections (e.g. an HNSW index from `ll-index`);
/// they are framed and CRC'd alongside the column chunks. Every column's value count and
/// `translation_table.len()` must equal `meta.row_count`.
pub fn write_file_with<P: AsRef<Path>>(
    path: P,
    meta: &FileMeta,
    columns: &[Column],
    translation_table: &[u64],
    extra: &[RawSection],
    opts: &WriteOptions,
) -> Result<()> {
    if translation_table.len() as u64 != meta.row_count {
        return Err(FormatError::Decode(format!(
            "translation_table length {} != row_count {}",
            translation_table.len(),
            meta.row_count
        )));
    }
    for col in columns {
        if col.values.len() as u64 != meta.row_count {
            return Err(FormatError::Decode(format!(
                "column {} has {} values but row_count is {}",
                col.column_id,
                col.values.len(),
                meta.row_count
            )));
        }
        if let ColumnValues::Vector { dim, data } = &col.values {
            for (i, x) in data.iter().enumerate() {
                if let Some(v) = x {
                    if v.len() != *dim as usize {
                        return Err(FormatError::Decode(format!(
                            "column {} row {} vector length {} != declared dim {}",
                            col.column_id,
                            i,
                            v.len(),
                            dim
                        )));
                    }
                }
            }
        }
    }

    // Opaque extras may only be the embedded-index kinds. ColumnChunk and TranslationTable
    // are written by the format itself; accepting them here would produce a file with
    // duplicate/conflicting structural sections.
    for sec in extra {
        if matches!(sec.kind, SectionType::ColumnChunk | SectionType::TranslationTable) {
            return Err(FormatError::Decode(format!(
                "extra section uses reserved kind {:?}; only Hnsw/Text/Edge/OptimizerStats \
                 may be passed as opaque extras",
                sec.kind
            )));
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut sections: Vec<SectionEntry> = Vec::new();
    let mut zone_maps = Vec::new();

    // [0] Preamble.
    let preamble = Preamble {
        file_uuid: meta.file_uuid,
        min_lsn: meta.min_lsn,
        max_lsn: meta.max_lsn,
        row_count: meta.row_count,
        schema_fingerprint: meta.schema_fingerprint,
        creation_unix_nanos: meta.creation_unix_nanos,
    };
    buf.extend_from_slice(&preamble.encode());
    debug_assert_eq!(buf.len(), PREAMBLE_LEN);

    // [1] Column-chunk sections.
    for col in columns {
        let encoded = encode_column_chunk(col, opts.rows_per_page);
        let offset = buf.len() as u64;
        let crc = crc32c(&encoded.bytes);
        buf.extend_from_slice(&encoded.bytes);
        sections.push(SectionEntry {
            kind: SectionType::ColumnChunk,
            encoding: SectionEncoding::Plain,
            column_id: col.column_id,
            offset,
            length: encoded.bytes.len() as u64,
            crc32: crc,
        });
        zone_maps.extend(encoded.zone_maps);
    }

    // [2] Opaque index/aux sections (HNSW/text/edge/optimizer-stats) supplied by callers.
    for sec in extra {
        let offset = buf.len() as u64;
        let crc = crc32c(&sec.bytes);
        buf.extend_from_slice(&sec.bytes);
        sections.push(SectionEntry {
            kind: sec.kind,
            encoding: sec.encoding,
            column_id: sec.column_id,
            offset,
            length: sec.bytes.len() as u64,
            crc32: crc,
        });
    }

    // [4] TranslationTable section (Plain u64 encoding in v0).
    let tt_offset = buf.len() as u64;
    let mut tt_body = Vec::with_capacity(translation_table.len() * 8);
    for &gid in translation_table {
        tt_body.extend_from_slice(&gid.to_le_bytes());
    }
    let tt_crc = crc32c(&tt_body);
    buf.extend_from_slice(&tt_body);
    sections.push(SectionEntry {
        kind: SectionType::TranslationTable,
        encoding: SectionEncoding::Plain,
        column_id: 0,
        offset: tt_offset,
        length: tt_body.len() as u64,
        crc32: tt_crc,
    });

    // [5] Footer.
    let footer = Footer {
        file_uuid: meta.file_uuid,
        format_version: FORMAT_VERSION,
        min_lsn: meta.min_lsn,
        max_lsn: meta.max_lsn,
        row_count: meta.row_count,
        schema_fingerprint: meta.schema_fingerprint,
        schema_blob: meta.schema_blob.clone(),
        sections,
        zone_maps,
        mvcc: meta.mvcc,
        writer_version: meta.writer_version.clone(),
    };
    let footer_bytes = footer.encode();
    let footer_crc = crc32c(&footer_bytes);
    buf.extend_from_slice(&footer_bytes);

    // [6] Trailer: footer_crc32 | footer_len(u64) | end_magic.
    buf.extend_from_slice(&footer_crc.to_le_bytes());
    buf.extend_from_slice(&(footer_bytes.len() as u64).to_le_bytes());
    buf.extend_from_slice(&MAGIC.to_le_bytes());

    write_atomic(path.as_ref(), &buf)
}

/// Read and validate a complete file.
pub fn read_file<P: AsRef<Path>>(path: P) -> Result<LlFile> {
    let bytes = fs::read(path.as_ref())?;
    let len = bytes.len();
    if len < PREAMBLE_LEN + TRAILER_LEN {
        return Err(FormatError::Truncated("file shorter than preamble+trailer"));
    }

    // [6] Trailer.
    let trailer = &bytes[len - TRAILER_LEN..];
    let footer_crc_expected = u32::from_le_bytes(trailer[0..4].try_into().unwrap());
    let footer_len = u64::from_le_bytes(trailer[4..12].try_into().unwrap()) as usize;
    let end_magic = u32::from_le_bytes(trailer[12..16].try_into().unwrap());
    if end_magic != MAGIC {
        return Err(FormatError::BadMagic {
            expected: MAGIC,
            found: end_magic,
        });
    }

    // [5] Footer.
    let footer_end = len - TRAILER_LEN;
    let footer_start = footer_end
        .checked_sub(footer_len)
        .ok_or(FormatError::Truncated("footer length exceeds file"))?;
    if footer_start < PREAMBLE_LEN {
        return Err(FormatError::Truncated("footer overlaps preamble"));
    }
    let footer_bytes = &bytes[footer_start..footer_end];
    let footer_crc_found = crc32c(footer_bytes);
    if footer_crc_found != footer_crc_expected {
        return Err(FormatError::FooterCrcMismatch {
            expected: footer_crc_expected,
            found: footer_crc_found,
        });
    }
    let footer = Footer::decode(footer_bytes)?;

    // [0] Preamble.
    let preamble = Preamble::decode(&bytes[0..PREAMBLE_LEN])?;
    if preamble.row_count != footer.row_count {
        return Err(FormatError::Decode(format!(
            "row_count mismatch: preamble {} vs footer {}",
            preamble.row_count, footer.row_count
        )));
    }
    if preamble.file_uuid != footer.file_uuid {
        return Err(FormatError::Decode(
            "file_uuid mismatch preamble vs footer".into(),
        ));
    }

    // [1] Column-chunk sections (in directory order), each CRC-verified then decoded.
    let mut columns = Vec::new();
    let mut raw_sections = Vec::new();
    for entry in &footer.sections {
        match entry.kind {
            SectionType::ColumnChunk => {
                let body = section_body(&bytes, entry, "ColumnChunk")?;
                columns.push(decode_column_chunk(body)?);
            }
            SectionType::Hnsw
            | SectionType::Text
            | SectionType::Edge
            | SectionType::OptimizerStats => {
                let body = section_body(&bytes, entry, "section")?;
                raw_sections.push(RawSection {
                    kind: entry.kind,
                    column_id: entry.column_id,
                    encoding: entry.encoding,
                    bytes: body.to_vec(),
                });
            }
            SectionType::TranslationTable => {}
        }
    }

    // [4] TranslationTable section.
    let tt_entry = footer
        .section(SectionType::TranslationTable)
        .ok_or_else(|| FormatError::Decode("missing TranslationTable section".into()))?;
    let tt = read_translation_table(&bytes, tt_entry, footer.row_count)?;

    Ok(LlFile {
        preamble,
        footer,
        columns,
        raw_sections,
        translation_table: tt,
    })
}

/// Slice a section's body, validating its bounds and CRC32C.
fn section_body<'a>(bytes: &'a [u8], entry: &SectionEntry, name: &str) -> Result<&'a [u8]> {
    let start = entry.offset as usize;
    let end = start
        .checked_add(entry.length as usize)
        .ok_or(FormatError::Truncated("section length overflow"))?;
    let body = bytes
        .get(start..end)
        .ok_or(FormatError::Truncated("section beyond file"))?;
    let found = crc32c(body);
    if found != entry.crc32 {
        return Err(FormatError::SectionCrcMismatch {
            section: name.to_string(),
            expected: entry.crc32,
            found,
        });
    }
    Ok(body)
}

fn read_translation_table(bytes: &[u8], entry: &SectionEntry, row_count: u64) -> Result<Vec<u64>> {
    let body = section_body(bytes, entry, "TranslationTable")?;
    if !body.len().is_multiple_of(8) {
        return Err(FormatError::Decode(
            "translation section not a multiple of 8 bytes".into(),
        ));
    }
    let count = body.len() / 8;
    if count as u64 != row_count {
        return Err(FormatError::Decode(format!(
            "translation table has {count} entries but row_count is {row_count}"
        )));
    }
    let mut out = Vec::with_capacity(count);
    for chunk in body.chunks_exact(8) {
        out.push(u64::from_le_bytes(chunk.try_into().unwrap()));
    }
    Ok(out)
}

/// Write `data` to `path` atomically: write to `<path>.tmp`, fsync, then rename.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let tmp: PathBuf = {
        let mut name = path
            .file_name()
            .ok_or_else(|| FormatError::Decode("path has no file name".into()))?
            .to_os_string();
        name.push(".tmp");
        path.with_file_name(name)
    };

    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    // fsync the parent directory so the rename itself survives a crash.
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}
