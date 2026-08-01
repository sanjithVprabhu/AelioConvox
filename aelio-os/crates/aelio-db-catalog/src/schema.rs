//! Schema types: a column's modality, column/table definitions, and a file reference.

/// A column's logical kind, which determines how it is indexed and queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Bool,
    I64,
    F64,
    /// Plain string (not fuaelio-db-text indexed).
    Utf8,
    Timestamp,
    /// Fixed-dimension vector (HNSW index).
    Vector(u16),
    /// Fuaelio-db-text indexed string (inverted index + BM25).
    Text,
    /// Graph edge column (forward/reverse CSR).
    Edge,
}

impl ColumnKind {
    pub(crate) fn tag(self) -> u8 {
        match self {
            ColumnKind::Bool => 0,
            ColumnKind::I64 => 1,
            ColumnKind::F64 => 2,
            ColumnKind::Utf8 => 3,
            ColumnKind::Timestamp => 4,
            ColumnKind::Vector(_) => 5,
            ColumnKind::Text => 6,
            ColumnKind::Edge => 7,
        }
    }

    pub(crate) fn dim(self) -> u16 {
        match self {
            ColumnKind::Vector(d) => d,
            _ => 0,
        }
    }

    pub(crate) fn from_parts(tag: u8, dim: u16) -> Option<Self> {
        Some(match tag {
            0 => ColumnKind::Bool,
            1 => ColumnKind::I64,
            2 => ColumnKind::F64,
            3 => ColumnKind::Utf8,
            4 => ColumnKind::Timestamp,
            5 => ColumnKind::Vector(dim),
            6 => ColumnKind::Text,
            7 => ColumnKind::Edge,
            _ => return None,
        })
    }

    /// Whether this column carries its own embedded index section (vector/text/edge).
    pub fn is_indexed_modality(self) -> bool {
        matches!(
            self,
            ColumnKind::Vector(_) | ColumnKind::Text | ColumnKind::Edge
        )
    }
}

/// A column definition. `column_id` is assigned by the catalog and never reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDef {
    pub column_id: u32,
    pub name: String,
    pub kind: ColumnKind,
}

/// A flushed `.vss` file belonging to a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub path: String,
    pub min_lsn: u64,
    pub max_lsn: u64,
    pub row_count: u64,
}

/// A table: its columns and the files that hold its persisted rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDef {
    pub table_id: u32,
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub files: Vec<FileRef>,
}

impl TableDef {
    pub fn column(&self, name: &str) -> Option<&ColumnDef> {
        self.columns.iter().find(|c| c.name == name)
    }
    pub fn column_by_id(&self, id: u32) -> Option<&ColumnDef> {
        self.columns.iter().find(|c| c.column_id == id)
    }
}
