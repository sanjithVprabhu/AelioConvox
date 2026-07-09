//! Cell values held in the memtable. A subset for v0; maps to `ll-format` column values on
//! flush.

/// A single column value in a row.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Utf8(String),
    Vector(Vec<f32>),
    /// Outgoing edges for an edge column: target global row ids.
    Edges(Vec<u64>),
}

impl Value {
    /// Rough in-memory byte size (for memtable sizing).
    pub fn approx_bytes(&self) -> usize {
        match self {
            Value::Null => 1,
            Value::Bool(_) => 1,
            Value::I64(_) => 8,
            Value::F64(_) => 8,
            Value::Utf8(s) => s.len() + 8,
            Value::Vector(v) => v.len() * 4 + 8,
            Value::Edges(e) => e.len() * 8 + 8,
        }
    }
}
