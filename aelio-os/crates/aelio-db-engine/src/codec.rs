//! Encode/decode a row operation as a WAL record payload.
//!
//! Put payload: `row_id u64 | ncols u32 | (col_id u32, value)*`. Delete payload: `row_id u64`.
//! A value is `tag u8` then its data.

use crate::memtable::{Op, Row};
use crate::value::Value;
use aelio_db_wal::RecordType;

const T_NULL: u8 = 0;
const T_BOOL: u8 = 1;
const T_I64: u8 = 2;
const T_F64: u8 = 3;
const T_UTF8: u8 = 4;
const T_VEC: u8 = 5;
const T_EDGES: u8 = 6;

pub fn encode_put(row_id: u64, row: &Row) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&row_id.to_le_bytes());
    b.extend_from_slice(&(row.len() as u32).to_le_bytes());
    for (cid, val) in row {
        b.extend_from_slice(&cid.to_le_bytes());
        encode_value(&mut b, val);
    }
    b
}

pub fn encode_delete(row_id: u64) -> Vec<u8> {
    row_id.to_le_bytes().to_vec()
}

fn encode_value(b: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => b.push(T_NULL),
        Value::Bool(x) => {
            b.push(T_BOOL);
            b.push(*x as u8);
        }
        Value::I64(x) => {
            b.push(T_I64);
            b.extend_from_slice(&x.to_le_bytes());
        }
        Value::F64(x) => {
            b.push(T_F64);
            b.extend_from_slice(&x.to_le_bytes());
        }
        Value::Utf8(s) => {
            b.push(T_UTF8);
            b.extend_from_slice(&(s.len() as u32).to_le_bytes());
            b.extend_from_slice(s.as_bytes());
        }
        Value::Vector(v) => {
            b.push(T_VEC);
            b.extend_from_slice(&(v.len() as u32).to_le_bytes());
            for f in v {
                b.extend_from_slice(&f.to_le_bytes());
            }
        }
        Value::Edges(e) => {
            b.push(T_EDGES);
            b.extend_from_slice(&(e.len() as u32).to_le_bytes());
            for t in e {
                b.extend_from_slice(&t.to_le_bytes());
            }
        }
    }
}

/// Decode a data record into `(row_id, op)`, or `None` for non-data record types.
pub fn decode(rtype: RecordType, payload: &[u8]) -> Option<(u64, Op)> {
    match rtype {
        RecordType::InsertRow | RecordType::UpdateRow => {
            let (row_id, row) = decode_put(payload)?;
            Some((row_id, Op::Put(row)))
        }
        RecordType::DeleteRow => Some((rd_u64(payload, 0)?, Op::Delete)),
        _ => None,
    }
}

fn decode_put(p: &[u8]) -> Option<(u64, Row)> {
    let row_id = rd_u64(p, 0)?;
    let ncols = rd_u32(p, 8)? as usize;
    let mut pos = 12;
    let mut row = Row::new();
    for _ in 0..ncols {
        let cid = rd_u32(p, pos)?;
        pos += 4;
        let (val, next) = decode_value(p, pos)?;
        pos = next;
        row.insert(cid, val);
    }
    Some((row_id, row))
}

fn decode_value(p: &[u8], pos: usize) -> Option<(Value, usize)> {
    let tag = *p.get(pos)?;
    let pos = pos + 1;
    Some(match tag {
        T_NULL => (Value::Null, pos),
        T_BOOL => (Value::Bool(*p.get(pos)? != 0), pos + 1),
        T_I64 => (
            Value::I64(i64::from_le_bytes(p.get(pos..pos + 8)?.try_into().unwrap())),
            pos + 8,
        ),
        T_F64 => (
            Value::F64(f64::from_le_bytes(p.get(pos..pos + 8)?.try_into().unwrap())),
            pos + 8,
        ),
        T_UTF8 => {
            let len = rd_u32(p, pos)? as usize;
            let start = pos + 4;
            let s = String::from_utf8(p.get(start..start + len)?.to_vec()).ok()?;
            (Value::Utf8(s), start + len)
        }
        T_VEC => {
            let dim = rd_u32(p, pos)? as usize;
            let start = pos + 4;
            let mut v = Vec::with_capacity(dim);
            for i in 0..dim {
                let o = start + i * 4;
                v.push(f32::from_le_bytes(p.get(o..o + 4)?.try_into().unwrap()));
            }
            (Value::Vector(v), start + dim * 4)
        }
        T_EDGES => {
            let count = rd_u32(p, pos)? as usize;
            let start = pos + 4;
            let mut e = Vec::with_capacity(count);
            for i in 0..count {
                let o = start + i * 8;
                e.push(u64::from_le_bytes(p.get(o..o + 8)?.try_into().unwrap()));
            }
            (Value::Edges(e), start + count * 8)
        }
        _ => return None,
    })
}

fn rd_u32(p: &[u8], o: usize) -> Option<u32> {
    p.get(o..o + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn rd_u64(p: &[u8], o: usize) -> Option<u64> {
    p.get(o..o + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}
