//! Scalar + vector logical types and the in-memory column value model used when writing
//! chunks. See `TechSpec/LL_FileFormat_ByteLayout.md` §2. Edge column types come later.

use crate::error::{FormatError, Result};

/// The logical type of a column. On-disk tag values are stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum LogicalType {
    Bool = 0,
    I32 = 3,
    I64 = 4,
    F32 = 10,
    F64 = 11,
    Utf8 = 20,
    /// Fixed-dimension `f32` vector (dimension carried in the chunk header).
    Vector = 30,
    /// 64-bit signed nanoseconds since the Unix epoch.
    TimestampNanos = 50,
}

impl LogicalType {
    pub fn from_u16(v: u16) -> Result<Self> {
        Ok(match v {
            0 => LogicalType::Bool,
            3 => LogicalType::I32,
            4 => LogicalType::I64,
            10 => LogicalType::F32,
            11 => LogicalType::F64,
            20 => LogicalType::Utf8,
            30 => LogicalType::Vector,
            50 => LogicalType::TimestampNanos,
            other => return Err(FormatError::Decode(format!("bad LogicalType tag {other}"))),
        })
    }

    /// Fixed serialized width in bytes for the simple scalar types. Returns `None` for
    /// variable-length (`Utf8`) and dimension-parameterized (`Vector`) types, which are
    /// handled specially by the chunk codec.
    pub fn fixed_width(self) -> Option<usize> {
        match self {
            LogicalType::Bool => Some(1),
            LogicalType::I32 | LogicalType::F32 => Some(4),
            LogicalType::I64 | LogicalType::F64 | LogicalType::TimestampNanos => Some(8),
            LogicalType::Utf8 | LogicalType::Vector => None,
        }
    }
}

/// Column data held in memory for writing. Each element is `Option<T>` so nulls are
/// represented naturally; a validity bitmap is emitted only when at least one value is
/// null. `Vector` rows are `Option<Vec<f32>>`, each `Some` having length `dim`.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnValues {
    Bool(Vec<Option<bool>>),
    I32(Vec<Option<i32>>),
    I64(Vec<Option<i64>>),
    F32(Vec<Option<f32>>),
    F64(Vec<Option<f64>>),
    Utf8(Vec<Option<String>>),
    Vector {
        dim: u16,
        data: Vec<Option<Vec<f32>>>,
    },
    TimestampNanos(Vec<Option<i64>>),
}

impl ColumnValues {
    pub fn logical_type(&self) -> LogicalType {
        match self {
            ColumnValues::Bool(_) => LogicalType::Bool,
            ColumnValues::I32(_) => LogicalType::I32,
            ColumnValues::I64(_) => LogicalType::I64,
            ColumnValues::F32(_) => LogicalType::F32,
            ColumnValues::F64(_) => LogicalType::F64,
            ColumnValues::Utf8(_) => LogicalType::Utf8,
            ColumnValues::Vector { .. } => LogicalType::Vector,
            ColumnValues::TimestampNanos(_) => LogicalType::TimestampNanos,
        }
    }

    /// Vector dimension, if this is a vector column.
    pub fn vector_dim(&self) -> Option<u16> {
        match self {
            ColumnValues::Vector { dim, .. } => Some(*dim),
            _ => None,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            ColumnValues::Bool(v) => v.len(),
            ColumnValues::I32(v) => v.len(),
            ColumnValues::I64(v) => v.len(),
            ColumnValues::F32(v) => v.len(),
            ColumnValues::F64(v) => v.len(),
            ColumnValues::Utf8(v) => v.len(),
            ColumnValues::Vector { data, .. } => data.len(),
            ColumnValues::TimestampNanos(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether any element in the column is null.
    pub fn has_nulls(&self) -> bool {
        match self {
            ColumnValues::Bool(v) => v.iter().any(|x| x.is_none()),
            ColumnValues::I32(v) => v.iter().any(|x| x.is_none()),
            ColumnValues::I64(v) => v.iter().any(|x| x.is_none()),
            ColumnValues::F32(v) => v.iter().any(|x| x.is_none()),
            ColumnValues::F64(v) => v.iter().any(|x| x.is_none()),
            ColumnValues::Utf8(v) => v.iter().any(|x| x.is_none()),
            ColumnValues::Vector { data, .. } => data.iter().any(|x| x.is_none()),
            ColumnValues::TimestampNanos(v) => v.iter().any(|x| x.is_none()),
        }
    }

    /// Total null count across the column.
    pub fn null_count(&self) -> u64 {
        let c = match self {
            ColumnValues::Bool(v) => v.iter().filter(|x| x.is_none()).count(),
            ColumnValues::I32(v) => v.iter().filter(|x| x.is_none()).count(),
            ColumnValues::I64(v) => v.iter().filter(|x| x.is_none()).count(),
            ColumnValues::F32(v) => v.iter().filter(|x| x.is_none()).count(),
            ColumnValues::F64(v) => v.iter().filter(|x| x.is_none()).count(),
            ColumnValues::Utf8(v) => v.iter().filter(|x| x.is_none()).count(),
            ColumnValues::Vector { data, .. } => data.iter().filter(|x| x.is_none()).count(),
            ColumnValues::TimestampNanos(v) => v.iter().filter(|x| x.is_none()).count(),
        };
        c as u64
    }

    /// Concatenate per-page segments (all of `lt`) into one column. For `Vector`, `dim`
    /// is taken from the segments (callers with 0 rows construct the column directly).
    pub fn concat(parts: Vec<ColumnValues>, lt: LogicalType) -> Result<Self> {
        macro_rules! merge {
            ($variant:ident) => {{
                let mut out = Vec::new();
                for p in parts {
                    match p {
                        ColumnValues::$variant(v) => out.extend(v),
                        other => return Err(type_mismatch(other.logical_type(), lt)),
                    }
                }
                ColumnValues::$variant(out)
            }};
        }
        Ok(match lt {
            LogicalType::Bool => merge!(Bool),
            LogicalType::I32 => merge!(I32),
            LogicalType::I64 => merge!(I64),
            LogicalType::F32 => merge!(F32),
            LogicalType::F64 => merge!(F64),
            LogicalType::Utf8 => merge!(Utf8),
            LogicalType::TimestampNanos => merge!(TimestampNanos),
            LogicalType::Vector => {
                let mut dim = 0u16;
                let mut out = Vec::new();
                for p in parts {
                    match p {
                        ColumnValues::Vector { dim: d, data } => {
                            dim = d;
                            out.extend(data);
                        }
                        other => return Err(type_mismatch(other.logical_type(), lt)),
                    }
                }
                ColumnValues::Vector { dim, data: out }
            }
        })
    }

    /// An empty column of the given logical type (vector dim 0 — set explicitly if known).
    pub fn empty(lt: LogicalType) -> Self {
        match lt {
            LogicalType::Bool => ColumnValues::Bool(Vec::new()),
            LogicalType::I32 => ColumnValues::I32(Vec::new()),
            LogicalType::I64 => ColumnValues::I64(Vec::new()),
            LogicalType::F32 => ColumnValues::F32(Vec::new()),
            LogicalType::F64 => ColumnValues::F64(Vec::new()),
            LogicalType::Utf8 => ColumnValues::Utf8(Vec::new()),
            LogicalType::Vector => ColumnValues::Vector {
                dim: 0,
                data: Vec::new(),
            },
            LogicalType::TimestampNanos => ColumnValues::TimestampNanos(Vec::new()),
        }
    }
}

fn type_mismatch(got: LogicalType, want: LogicalType) -> FormatError {
    FormatError::Decode(format!(
        "page type {got:?} does not match chunk type {want:?}"
    ))
}
