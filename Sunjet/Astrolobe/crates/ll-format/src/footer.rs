//! The footer (region [5]) — the file's table of contents — and its section directory.
//!
//! Structure mirrors `footer.fbs` field-for-field. Serialized here with a hand-written
//! little-endian codec (no FlatBuffers dependency yet); see `footer.fbs` for the
//! rationale and the migration path.

use crate::codec::{Reader, Writer};
use crate::error::{FormatError, Result};
use crate::preamble::FORMAT_VERSION;

/// Internal version of the manual footer codec (independent of on-disk format_version).
const FOOTER_CODEC_VERSION: u16 = 1;

/// What a section in the file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SectionType {
    ColumnChunk = 0,
    Hnsw = 1,
    Text = 2,
    Edge = 3,
    OptimizerStats = 4,
    TranslationTable = 5,
}

impl SectionType {
    fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            0 => SectionType::ColumnChunk,
            1 => SectionType::Hnsw,
            2 => SectionType::Text,
            3 => SectionType::Edge,
            4 => SectionType::OptimizerStats,
            5 => SectionType::TranslationTable,
            other => return Err(FormatError::Decode(format!("bad SectionType tag {other}"))),
        })
    }
}

/// How a section's bytes are encoded (relevant to TranslationTable and future columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SectionEncoding {
    Plain = 0,
    Delta = 1,
    Pfor = 2,
}

impl SectionEncoding {
    fn from_u8(v: u8) -> Result<Self> {
        Ok(match v {
            0 => SectionEncoding::Plain,
            1 => SectionEncoding::Delta,
            2 => SectionEncoding::Pfor,
            other => return Err(FormatError::Decode(format!("bad SectionEncoding tag {other}"))),
        })
    }
}

/// One entry in the footer's section directory: where a section lives and its checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionEntry {
    pub kind: SectionType,
    pub encoding: SectionEncoding,
    pub column_id: u32,
    /// Absolute byte offset of the section body within the file.
    pub offset: u64,
    /// Length of the section body in bytes.
    pub length: u64,
    /// CRC32C over the section body.
    pub crc32: u32,
}

/// Per-column, per-page statistics used for predicate pushdown / file & page pruning.
/// `min`/`max` are typed little-endian bytes (empty when the page has no non-null value).
/// `centroid`/`max_radius` are populated for vector columns (empty/0 for scalars).
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneMap {
    pub column_id: u32,
    pub page_index: u32,
    pub min: Vec<u8>,
    pub max: Vec<u8>,
    pub null_count: u64,
    pub distinct_estimate: u64,
    pub total: u64,
    pub centroid: Vec<f32>,
    pub max_radius: f32,
}

/// Snapshot of MVCC bounds across the file, for fast snapshot pruning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MvccSummary {
    pub min_xmin: u64,
    pub max_xmax: u64,
    pub tombstone_count: u64,
    pub live_count: u64,
}

/// The file footer. `zone_maps`, `blooms`, and `compression_dicts` are reserved (empty)
/// in v0 but encoded as explicit zero counts so the layout is forward-stable.
#[derive(Debug, Clone, PartialEq)]
pub struct Footer {
    pub file_uuid: [u8; 16],
    pub format_version: u16,
    pub min_lsn: u64,
    pub max_lsn: u64,
    pub row_count: u64,
    pub schema_fingerprint: u64,
    pub schema_blob: Vec<u8>,
    pub sections: Vec<SectionEntry>,
    pub zone_maps: Vec<ZoneMap>,
    pub mvcc: MvccSummary,
    pub writer_version: String,
}

impl Footer {
    /// Find the first section of a given type.
    pub fn section(&self, kind: SectionType) -> Option<&SectionEntry> {
        self.sections.iter().find(|s| s.kind == kind)
    }

    /// Serialize the footer to bytes (the blob whose length/CRC live in the trailer).
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u16(FOOTER_CODEC_VERSION);
        w.bytes(&self.file_uuid);
        w.u16(self.format_version);
        w.u64(self.min_lsn);
        w.u64(self.max_lsn);
        w.u64(self.row_count);
        w.u64(self.schema_fingerprint);
        w.lp_bytes(&self.schema_blob);

        w.u32(self.sections.len() as u32);
        for s in &self.sections {
            w.u8(s.kind as u8);
            w.u8(s.encoding as u8);
            w.u32(s.column_id);
            w.u64(s.offset);
            w.u64(s.length);
            w.u32(s.crc32);
        }

        w.u32(self.zone_maps.len() as u32);
        for z in &self.zone_maps {
            w.u32(z.column_id);
            w.u32(z.page_index);
            w.lp_bytes(&z.min);
            w.lp_bytes(&z.max);
            w.u64(z.null_count);
            w.u64(z.distinct_estimate);
            w.u64(z.total);
            w.u32(z.centroid.len() as u32);
            for f in &z.centroid {
                w.u32(f.to_bits());
            }
            w.u32(z.max_radius.to_bits());
        }

        // Reserved collections (v0): explicit zero counts.
        w.u32(0); // blooms
        w.u32(0); // compression_dicts

        w.u64(self.mvcc.min_xmin);
        w.u64(self.mvcc.max_xmax);
        w.u64(self.mvcc.tombstone_count);
        w.u64(self.mvcc.live_count);

        w.lp_str(&self.writer_version);
        w.buf
    }

    /// Parse a footer from its serialized bytes.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let mut r = Reader::new(buf);
        let codec_version = r.u16()?;
        if codec_version != FOOTER_CODEC_VERSION {
            return Err(FormatError::Decode(format!(
                "unsupported footer codec version {codec_version}"
            )));
        }
        let mut file_uuid = [0u8; 16];
        file_uuid.copy_from_slice(&r_bytes(&mut r, 16)?);
        let format_version = r.u16()?;
        if format_version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(format_version));
        }
        let min_lsn = r.u64()?;
        let max_lsn = r.u64()?;
        let row_count = r.u64()?;
        let schema_fingerprint = r.u64()?;
        let schema_blob = r.lp_bytes()?;

        let section_count = r.u32()? as usize;
        let mut sections = Vec::with_capacity(section_count);
        for _ in 0..section_count {
            let kind = SectionType::from_u8(r.u8()?)?;
            let encoding = SectionEncoding::from_u8(r.u8()?)?;
            let column_id = r.u32()?;
            let offset = r.u64()?;
            let length = r.u64()?;
            let crc32 = r.u32()?;
            sections.push(SectionEntry {
                kind,
                encoding,
                column_id,
                offset,
                length,
                crc32,
            });
        }

        let zone_map_count = r.u32()? as usize;
        let mut zone_maps = Vec::with_capacity(zone_map_count);
        for _ in 0..zone_map_count {
            let column_id = r.u32()?;
            let page_index = r.u32()?;
            let min = r.lp_bytes()?;
            let max = r.lp_bytes()?;
            let null_count = r.u64()?;
            let distinct_estimate = r.u64()?;
            let total = r.u64()?;
            let centroid_len = r.u32()? as usize;
            let mut centroid = Vec::with_capacity(centroid_len);
            for _ in 0..centroid_len {
                centroid.push(f32::from_bits(r.u32()?));
            }
            let max_radius = f32::from_bits(r.u32()?);
            zone_maps.push(ZoneMap {
                column_id,
                page_index,
                min,
                max,
                null_count,
                distinct_estimate,
                total,
                centroid,
                max_radius,
            });
        }

        // Reserved collections — must currently be empty.
        for name in ["blooms", "compression_dicts"] {
            let n = r.u32()?;
            if n != 0 {
                return Err(FormatError::Decode(format!(
                    "v0 reader expects 0 {name}, found {n}"
                )));
            }
        }

        let mvcc = MvccSummary {
            min_xmin: r.u64()?,
            max_xmax: r.u64()?,
            tombstone_count: r.u64()?,
            live_count: r.u64()?,
        };
        let writer_version = r.lp_str()?;

        Ok(Footer {
            file_uuid,
            format_version,
            min_lsn,
            max_lsn,
            row_count,
            schema_fingerprint,
            schema_blob,
            sections,
            zone_maps,
            mvcc,
            writer_version,
        })
    }
}

/// Read exactly `n` bytes as an owned vec (small helper for the fixed UUID field).
fn r_bytes(r: &mut Reader<'_>, n: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(r.u8()?);
    }
    Ok(out)
}
