//! Scalar column-chunk encode/decode (plain encoding, v0). See byte-layout §2.
//!
//! A chunk's bytes (one [`crate::SectionType::ColumnChunk`] section) are:
//! `[chunk header][page 0..N][page index]`. Each page is independently CRC'd; the whole
//! chunk is CRC'd again at the section level by the footer's `SectionEntry`.

use crate::checksum::crc32c;
use crate::codec::{Reader, Writer};
use crate::error::{FormatError, Result};
use crate::footer::ZoneMap;
use crate::types::{ColumnValues, LogicalType};

/// Fixed chunk-header length (see field list below).
const CHUNK_HEADER_LEN: usize = 50;
/// Fixed per-page header length.
const PAGE_HEADER_LEN: usize = 16;
/// Fixed page-index entry length.
const PAGE_INDEX_ENTRY_LEN: usize = 24;

const ENCODING_PLAIN: u8 = 0;
const COMPRESSION_NONE: u8 = 0;
const PAGE_TYPE_COLUMN: u8 = 0;

const FLAG_NULLABLE: u16 = 1 << 0;
const FLAG_IS_SYSTEM: u16 = 1 << 3;

/// Default rows per page when the caller does not override it.
pub const DEFAULT_ROWS_PER_PAGE: u32 = 8192;

/// A column to be written: its catalog id, system flag, and in-memory values.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub column_id: u32,
    pub is_system: bool,
    pub values: ColumnValues,
}

/// The serialized chunk plus the zone maps the writer should hand to the footer.
pub(crate) struct EncodedChunk {
    pub bytes: Vec<u8>,
    pub zone_maps: Vec<ZoneMap>,
}

/// Encode one column into chunk bytes + per-page zone maps.
pub(crate) fn encode_column_chunk(col: &Column, rows_per_page: u32) -> EncodedChunk {
    let lt = col.values.logical_type();
    let n = col.values.len();
    let nullable = col.values.has_nulls();
    let rpp = (rows_per_page.max(1)) as usize;
    let num_pages = if n == 0 { 0 } else { n.div_ceil(rpp) };

    let mut pages: Vec<u8> = Vec::new();
    let mut index: Vec<(u64, u32, u32, u32, u32)> = Vec::new(); // offset, clen, ulen, rows, first
    let mut zone_maps: Vec<ZoneMap> = Vec::new();

    for p in 0..num_pages {
        let s = p * rpp;
        let e = (s + rpp).min(n);
        let rows = (e - s) as u32;

        let payload = encode_page_payload(&col.values, s, e, nullable);
        let ulen = payload.len() as u32;
        let clen = ulen; // no compression in v0
        let crc = crc32c(&payload);

        let page_offset = (CHUNK_HEADER_LEN + pages.len()) as u64;
        // page header
        pages.push(PAGE_TYPE_COLUMN);
        pages.push(COMPRESSION_NONE);
        pages.extend_from_slice(&0u16.to_le_bytes()); // reserved
        pages.extend_from_slice(&ulen.to_le_bytes());
        pages.extend_from_slice(&clen.to_le_bytes());
        pages.extend_from_slice(&crc.to_le_bytes());
        pages.extend_from_slice(&payload);

        index.push((page_offset, clen, ulen, rows, s as u32));

        let (min, max, centroid, max_radius) = if lt == LogicalType::Vector {
            let (c, r) = vector_centroid(&col.values, s, e);
            (Vec::new(), Vec::new(), c, r)
        } else {
            let (mn, mx) = minmax(&col.values, s, e);
            (mn, mx, Vec::new(), 0.0)
        };
        zone_maps.push(ZoneMap {
            column_id: col.column_id,
            page_index: p as u32,
            min,
            max,
            null_count: null_count_range(&col.values, s, e),
            distinct_estimate: 0, // unknown in v0
            total: rows as u64,
            centroid,
            max_radius,
        });
    }

    let page_index_offset = (CHUNK_HEADER_LEN + pages.len()) as u64;
    let mut index_bytes = Vec::with_capacity(index.len() * PAGE_INDEX_ENTRY_LEN);
    for (off, clen, ulen, rows, first) in &index {
        index_bytes.extend_from_slice(&off.to_le_bytes());
        index_bytes.extend_from_slice(&clen.to_le_bytes());
        index_bytes.extend_from_slice(&ulen.to_le_bytes());
        index_bytes.extend_from_slice(&rows.to_le_bytes());
        index_bytes.extend_from_slice(&first.to_le_bytes());
    }

    let total_bytes = CHUNK_HEADER_LEN as u64 + pages.len() as u64 + index_bytes.len() as u64;

    let mut flags = 0u16;
    if nullable {
        flags |= FLAG_NULLABLE;
    }
    if col.is_system {
        flags |= FLAG_IS_SYSTEM;
    }

    let mut w = Writer::new();
    w.u32(col.column_id);
    w.u16(lt as u16);
    w.u8(ENCODING_PLAIN);
    w.u8(COMPRESSION_NONE);
    w.u16(flags);
    w.u16(0); // vector_quant (0 = f32 full precision; int8 lives in the HNSW section)
    w.u16(col.values.vector_dim().unwrap_or(0));
    w.u64(n as u64); // value_count
    w.u64(col.values.null_count());
    w.u32(num_pages as u32);
    w.u64(page_index_offset);
    w.u64(total_bytes);
    debug_assert_eq!(w.buf.len(), CHUNK_HEADER_LEN);

    let mut bytes = w.buf;
    bytes.extend_from_slice(&pages);
    bytes.extend_from_slice(&index_bytes);
    EncodedChunk { bytes, zone_maps }
}

/// Decode a column chunk from its section bytes, verifying every page CRC.
pub(crate) fn decode_column_chunk(bytes: &[u8]) -> Result<Column> {
    let mut r = Reader::new(bytes);
    let column_id = r.u32()?;
    let lt = LogicalType::from_u16(r.u16()?)?;
    let encoding = r.u8()?;
    let _compression = r.u8()?;
    let flags = r.u16()?;
    let _vector_quant = r.u16()?;
    let vector_dim = r.u16()?;
    let value_count = r.u64()? as usize;
    let _null_count = r.u64()?;
    let num_pages = r.u32()? as usize;
    let page_index_offset = r.u64()? as usize;
    let _total_bytes = r.u64()?;

    if encoding != ENCODING_PLAIN {
        return Err(FormatError::Decode(format!(
            "unsupported column encoding {encoding} (only plain in v0)"
        )));
    }
    let nullable = flags & FLAG_NULLABLE != 0;
    let is_system = flags & FLAG_IS_SYSTEM != 0;

    let mut segments = Vec::with_capacity(num_pages);
    for p in 0..num_pages {
        let entry_off = page_index_offset + p * PAGE_INDEX_ENTRY_LEN;
        let entry = take(bytes, entry_off, PAGE_INDEX_ENTRY_LEN)?;
        let page_offset = u64::from_le_bytes(entry[0..8].try_into().unwrap()) as usize;
        let compressed_len = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let _uncompressed_len = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let rows = u32::from_le_bytes(entry[16..20].try_into().unwrap()) as usize;
        let _first_local = u32::from_le_bytes(entry[20..24].try_into().unwrap());

        let header = take(bytes, page_offset, PAGE_HEADER_LEN)?;
        let stored_crc = u32::from_le_bytes(header[12..16].try_into().unwrap());
        let payload = take(bytes, page_offset + PAGE_HEADER_LEN, compressed_len)?;
        let found_crc = crc32c(payload);
        if found_crc != stored_crc {
            return Err(FormatError::PageCrcMismatch {
                column_id,
                page_index: p as u32,
                expected: stored_crc,
                found: found_crc,
            });
        }
        segments.push(decode_page_payload(
            payload, rows, nullable, lt, vector_dim,
        )?);
    }

    let values = if lt == LogicalType::Vector {
        // Build directly so the dim from the header is authoritative even with 0 rows.
        let mut data = Vec::new();
        for seg in segments {
            match seg {
                ColumnValues::Vector { data: d, .. } => data.extend(d),
                other => {
                    return Err(FormatError::Decode(format!(
                        "page type {:?} in vector chunk",
                        other.logical_type()
                    )))
                }
            }
        }
        ColumnValues::Vector {
            dim: vector_dim,
            data,
        }
    } else {
        ColumnValues::concat(segments, lt)?
    };
    if values.len() != value_count {
        return Err(FormatError::Decode(format!(
            "decoded {} values but header says {}",
            values.len(),
            value_count
        )));
    }
    Ok(Column {
        column_id,
        is_system,
        values,
    })
}

// ---- page payload (de)serialization ------------------------------------------------

fn is_some_at(col: &ColumnValues, i: usize) -> bool {
    match col {
        ColumnValues::Bool(v) => v[i].is_some(),
        ColumnValues::I32(v) => v[i].is_some(),
        ColumnValues::I64(v) => v[i].is_some(),
        ColumnValues::F32(v) => v[i].is_some(),
        ColumnValues::F64(v) => v[i].is_some(),
        ColumnValues::Utf8(v) => v[i].is_some(),
        ColumnValues::Vector { data, .. } => data[i].is_some(),
        ColumnValues::TimestampNanos(v) => v[i].is_some(),
    }
}

fn pack_validity(col: &ColumnValues, s: usize, e: usize) -> Vec<u8> {
    let n = e - s;
    let mut bm = vec![0u8; n.div_ceil(8)];
    for j in 0..n {
        if is_some_at(col, s + j) {
            bm[j / 8] |= 1 << (j % 8);
        }
    }
    bm
}

fn encode_page_payload(col: &ColumnValues, s: usize, e: usize, nullable: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if nullable {
        out.extend_from_slice(&pack_validity(col, s, e));
    }
    match col {
        ColumnValues::Bool(v) => {
            for x in &v[s..e] {
                out.push(x.unwrap_or(false) as u8);
            }
        }
        ColumnValues::I32(v) => {
            for x in &v[s..e] {
                out.extend_from_slice(&x.unwrap_or(0).to_le_bytes());
            }
        }
        ColumnValues::I64(v) | ColumnValues::TimestampNanos(v) => {
            for x in &v[s..e] {
                out.extend_from_slice(&x.unwrap_or(0).to_le_bytes());
            }
        }
        ColumnValues::F32(v) => {
            for x in &v[s..e] {
                out.extend_from_slice(&x.unwrap_or(0.0).to_le_bytes());
            }
        }
        ColumnValues::F64(v) => {
            for x in &v[s..e] {
                out.extend_from_slice(&x.unwrap_or(0.0).to_le_bytes());
            }
        }
        ColumnValues::Utf8(v) => {
            let m = e - s;
            let mut data: Vec<u8> = Vec::new();
            let mut offsets: Vec<u32> = Vec::with_capacity(m + 1);
            offsets.push(0);
            for x in &v[s..e] {
                if let Some(string) = x {
                    data.extend_from_slice(string.as_bytes());
                }
                offsets.push(data.len() as u32);
            }
            for o in offsets {
                out.extend_from_slice(&o.to_le_bytes());
            }
            out.extend_from_slice(&data);
        }
        ColumnValues::Vector { dim, data } => {
            let d = *dim as usize;
            for x in &data[s..e] {
                match x {
                    Some(v) => {
                        debug_assert_eq!(v.len(), d);
                        for c in v {
                            out.extend_from_slice(&c.to_le_bytes());
                        }
                    }
                    None => out.extend(std::iter::repeat_n(0u8, d * 4)),
                }
            }
        }
    }
    out
}

fn decode_page_payload(
    buf: &[u8],
    m: usize,
    nullable: bool,
    lt: LogicalType,
    dim: u16,
) -> Result<ColumnValues> {
    let mut pos = 0usize;
    let valid: Vec<bool> = if nullable {
        let nb = m.div_ceil(8);
        let bm = take(buf, pos, nb)?;
        pos += nb;
        (0..m).map(|j| (bm[j / 8] >> (j % 8)) & 1 == 1).collect()
    } else {
        vec![true; m]
    };

    if lt == LogicalType::Utf8 {
        let off_bytes = take(buf, pos, 4 * (m + 1))?;
        pos += 4 * (m + 1);
        let offsets: Vec<u32> = (0..=m)
            .map(|i| u32::from_le_bytes(off_bytes[i * 4..i * 4 + 4].try_into().unwrap()))
            .collect();
        let data = &buf[pos..];
        let mut out = Vec::with_capacity(m);
        for (j, valid_j) in valid.iter().enumerate() {
            if *valid_j {
                let a = offsets[j] as usize;
                let b = offsets[j + 1] as usize;
                let slice = data
                    .get(a..b)
                    .ok_or(FormatError::Truncated("utf8 page data"))?;
                let string = String::from_utf8(slice.to_vec())
                    .map_err(|err| FormatError::Decode(format!("invalid utf-8: {err}")))?;
                out.push(Some(string));
            } else {
                out.push(None);
            }
        }
        return Ok(ColumnValues::Utf8(out));
    }

    if lt == LogicalType::Vector {
        let d = dim as usize;
        let w = d * 4;
        let vals = take(buf, pos, w * m)?;
        let mut out = Vec::with_capacity(m);
        for (j, valid_j) in valid.iter().enumerate() {
            if *valid_j {
                let base = j * w;
                let mut row = Vec::with_capacity(d);
                for k in 0..d {
                    let off = base + k * 4;
                    row.push(f32::from_le_bytes(vals[off..off + 4].try_into().unwrap()));
                }
                out.push(Some(row));
            } else {
                out.push(None);
            }
        }
        return Ok(ColumnValues::Vector { dim, data: out });
    }

    let w = lt
        .fixed_width()
        .expect("non-utf8/non-vector type has fixed width");
    let vals = take(buf, pos, w * m)?;
    macro_rules! build {
        ($variant:ident, $ty:ty, $from:expr) => {{
            let mut out = Vec::with_capacity(m);
            for (j, valid_j) in valid.iter().enumerate() {
                if *valid_j {
                    let chunk = &vals[j * w..j * w + w];
                    out.push(Some($from(chunk)));
                } else {
                    out.push(None);
                }
            }
            ColumnValues::$variant(out)
        }};
    }
    Ok(match lt {
        LogicalType::Bool => build!(Bool, bool, |c: &[u8]| c[0] != 0),
        LogicalType::I32 => build!(I32, i32, |c: &[u8]| i32::from_le_bytes(
            c.try_into().unwrap()
        )),
        LogicalType::I64 => build!(I64, i64, |c: &[u8]| i64::from_le_bytes(
            c.try_into().unwrap()
        )),
        LogicalType::F32 => build!(F32, f32, |c: &[u8]| f32::from_le_bytes(
            c.try_into().unwrap()
        )),
        LogicalType::F64 => build!(F64, f64, |c: &[u8]| f64::from_le_bytes(
            c.try_into().unwrap()
        )),
        LogicalType::TimestampNanos => {
            build!(TimestampNanos, i64, |c: &[u8]| i64::from_le_bytes(
                c.try_into().unwrap()
            ))
        }
        LogicalType::Utf8 | LogicalType::Vector => unreachable!("handled above"),
    })
}

// ---- zone-map statistics -----------------------------------------------------------

fn null_count_range(col: &ColumnValues, s: usize, e: usize) -> u64 {
    (s..e).filter(|&i| !is_some_at(col, i)).count() as u64
}

/// Compute typed min/max over the non-null values in `[s, e)`. Returns empty byte
/// vectors when the range has no non-null value. Floats ignore NaN.
fn minmax(col: &ColumnValues, s: usize, e: usize) -> (Vec<u8>, Vec<u8>) {
    macro_rules! ord_minmax {
        ($v:expr, $rng:expr) => {{
            let mut lo = None;
            let mut hi = None;
            for x in &$v[$rng] {
                if let Some(val) = x {
                    lo = Some(match lo {
                        None => *val,
                        Some(cur) if *val < cur => *val,
                        Some(cur) => cur,
                    });
                    hi = Some(match hi {
                        None => *val,
                        Some(cur) if *val > cur => *val,
                        Some(cur) => cur,
                    });
                }
            }
            (lo, hi)
        }};
    }
    macro_rules! float_minmax {
        ($v:expr, $rng:expr) => {{
            let mut lo: Option<_> = None;
            let mut hi: Option<_> = None;
            for x in &$v[$rng] {
                if let Some(val) = x {
                    if val.is_nan() {
                        continue;
                    }
                    lo = Some(match lo {
                        None => *val,
                        Some(cur) if *val < cur => *val,
                        Some(cur) => cur,
                    });
                    hi = Some(match hi {
                        None => *val,
                        Some(cur) if *val > cur => *val,
                        Some(cur) => cur,
                    });
                }
            }
            (lo, hi)
        }};
    }

    match col {
        ColumnValues::Bool(v) => {
            let (lo, hi) = ord_minmax!(v, s..e);
            match (lo, hi) {
                (Some(l), Some(h)) => (vec![l as u8], vec![h as u8]),
                _ => (vec![], vec![]),
            }
        }
        ColumnValues::I32(v) => {
            let (lo, hi) = ord_minmax!(v, s..e);
            bytes_pair(
                lo.map(|x| x.to_le_bytes().to_vec()),
                hi.map(|x| x.to_le_bytes().to_vec()),
            )
        }
        ColumnValues::I64(v) | ColumnValues::TimestampNanos(v) => {
            let (lo, hi) = ord_minmax!(v, s..e);
            bytes_pair(
                lo.map(|x| x.to_le_bytes().to_vec()),
                hi.map(|x| x.to_le_bytes().to_vec()),
            )
        }
        ColumnValues::F32(v) => {
            let (lo, hi) = float_minmax!(v, s..e);
            bytes_pair(
                lo.map(|x| x.to_le_bytes().to_vec()),
                hi.map(|x| x.to_le_bytes().to_vec()),
            )
        }
        ColumnValues::F64(v) => {
            let (lo, hi) = float_minmax!(v, s..e);
            bytes_pair(
                lo.map(|x| x.to_le_bytes().to_vec()),
                hi.map(|x| x.to_le_bytes().to_vec()),
            )
        }
        ColumnValues::Utf8(v) => {
            let mut lo: Option<&str> = None;
            let mut hi: Option<&str> = None;
            for string in v[s..e].iter().flatten() {
                let val = string.as_str();
                lo = Some(match lo {
                    None => val,
                    Some(cur) if val < cur => val,
                    Some(cur) => cur,
                });
                hi = Some(match hi {
                    None => val,
                    Some(cur) if val > cur => val,
                    Some(cur) => cur,
                });
            }
            bytes_pair(
                lo.map(|x| x.as_bytes().to_vec()),
                hi.map(|x| x.as_bytes().to_vec()),
            )
        }
        // Vectors use centroid/max_radius zone maps instead of min/max.
        ColumnValues::Vector { .. } => (vec![], vec![]),
    }
}

/// Per-page vector zone map: the mean vector (centroid) and the maximum L2 distance from
/// any non-null vector to that centroid. Empty/0 when the page has no non-null vector.
fn vector_centroid(col: &ColumnValues, s: usize, e: usize) -> (Vec<f32>, f32) {
    let ColumnValues::Vector { dim, data } = col else {
        return (Vec::new(), 0.0);
    };
    let d = *dim as usize;
    let mut sum = vec![0f64; d];
    let mut count = 0u64;
    for v in data[s..e].iter().flatten() {
        for (k, c) in v.iter().enumerate() {
            sum[k] += *c as f64;
        }
        count += 1;
    }
    if count == 0 {
        return (Vec::new(), 0.0);
    }
    let centroid: Vec<f32> = sum.iter().map(|x| (x / count as f64) as f32).collect();
    let mut max_radius = 0f32;
    for v in data[s..e].iter().flatten() {
        let mut acc = 0f64;
        for (k, c) in v.iter().enumerate() {
            let diff = *c as f64 - centroid[k] as f64;
            acc += diff * diff;
        }
        let r = acc.sqrt() as f32;
        if r > max_radius {
            max_radius = r;
        }
    }
    (centroid, max_radius)
}

fn bytes_pair(lo: Option<Vec<u8>>, hi: Option<Vec<u8>>) -> (Vec<u8>, Vec<u8>) {
    match (lo, hi) {
        (Some(l), Some(h)) => (l, h),
        _ => (vec![], vec![]),
    }
}

/// Bounds-checked slice.
fn take(buf: &[u8], off: usize, len: usize) -> Result<&[u8]> {
    let end = off
        .checked_add(len)
        .ok_or(FormatError::Truncated("chunk length overflow"))?;
    buf.get(off..end)
        .ok_or(FormatError::Truncated("chunk region beyond bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ColumnValues;

    fn col(column_id: u32, values: ColumnValues) -> Column {
        Column {
            column_id,
            is_system: false,
            values,
        }
    }

    #[test]
    fn chunk_roundtrip_i64_multipage() {
        let values = ColumnValues::I64((0..20).map(|i| Some(i as i64)).collect());
        let c = col(7, values.clone());
        let enc = encode_column_chunk(&c, 7);
        let dec = decode_column_chunk(&enc.bytes).unwrap();
        assert_eq!(dec.values, values);
        assert_eq!(dec.column_id, 7);
        assert_eq!(enc.zone_maps.len(), 3); // ceil(20/7)
    }

    #[test]
    fn chunk_roundtrip_utf8_with_nulls() {
        let values = ColumnValues::Utf8(vec![Some("a".into()), None, Some("ccc".into()), None]);
        let c = col(2, values.clone());
        let enc = encode_column_chunk(&c, 100);
        let dec = decode_column_chunk(&enc.bytes).unwrap();
        assert_eq!(dec.values, values);
    }

    #[test]
    fn chunk_page_crc_detects_corruption() {
        let c = col(1, ColumnValues::I64((0..10).map(Some).collect()));
        let mut enc = encode_column_chunk(&c, 100);
        // Flip a byte inside the (single) page payload: header(50) + page header(16) + 2.
        enc.bytes[50 + 16 + 2] ^= 0xFF;
        let err = decode_column_chunk(&enc.bytes).unwrap_err();
        assert!(
            matches!(err, FormatError::PageCrcMismatch { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn empty_column_has_no_pages() {
        let enc = encode_column_chunk(&col(1, ColumnValues::I64(vec![])), 100);
        let dec = decode_column_chunk(&enc.bytes).unwrap();
        assert!(dec.values.is_empty());
        assert!(enc.zone_maps.is_empty());
    }
}
